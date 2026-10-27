//! The files of a cover, from a folder or a zip archive alike, found by
//! name whatever their case: covers are made on Windows, where `Sounds/A.wav`
//! and `sounds/a.wav` are the same file.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// How deep a folder is looked into, against links that loop.
const MAX_DEPTH: usize = 16;

/// The files of a cover.
pub trait FileSource: Send + Sync {
    /// Every file, by its path from the top with `/` between folders.
    fn listing(&self) -> &Listing;

    /// The contents of a file, by its path exactly as listed.
    fn read_listed(&self, path: &str) -> io::Result<Vec<u8>>;

    /// What the files are, for messages: the folder or archive.
    fn origin(&self) -> &Path;
}

impl dyn FileSource + '_ {
    /// The listed path of a file, whatever the case it is asked in.
    pub fn find(&self, path: &str) -> Option<&str> {
        self.listing().find(path)
    }

    /// The contents of a file, whatever the case it is asked in.
    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let listed = self.find(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} is not in the cover"),
            )
        })?;
        self.read_listed(listed)
    }
}

/// The paths of the files, and an index of them in lower case.
#[derive(Debug, Default)]
pub struct Listing {
    paths: Vec<String>,
    lower: HashMap<String, usize>,
}

impl Listing {
    pub(crate) fn new(paths: Vec<String>) -> Self {
        let mut lower = HashMap::with_capacity(paths.len());
        for (i, path) in paths.iter().enumerate() {
            // The first of two that differ only by case wins, as on Windows
            // only one of them could have been there.
            lower.entry(normalise(path)).or_insert(i);
        }
        Listing { paths, lower }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn find(&self, path: &str) -> Option<&str> {
        self.lower
            .get(&normalise(path))
            .map(|&i| self.paths[i].as_str())
    }
}

/// A path as it is looked up: in lower case, with `/` between folders and
/// none at the start.
pub fn normalise(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let mut path = path.as_str();
    loop {
        if let Some(rest) = path.strip_prefix("./") {
            path = rest;
        } else if let Some(rest) = path.strip_prefix('/') {
            path = rest;
        } else {
            break;
        }
    }
    path.to_owned()
}

/// The files of a folder, or of a zip archive.
pub fn open(path: &Path) -> io::Result<Arc<dyn FileSource>> {
    if path.is_dir() {
        Ok(Arc::new(DirSource::open(path)?))
    } else {
        Ok(Arc::new(ZipSource::open(path)?))
    }
}

pub struct DirSource {
    root: PathBuf,
    listing: Listing,
}

impl DirSource {
    pub fn open(root: &Path) -> io::Result<Self> {
        let mut paths = Vec::new();
        walk(root, "", 0, &mut paths)?;
        paths.sort();
        Ok(DirSource {
            root: root.to_owned(),
            listing: Listing::new(paths),
        })
    }
}

fn walk(dir: &Path, prefix: &str, depth: usize, into: &mut Vec<String>) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        // Through links, as a cover may well be put together with them.
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        if metadata.is_dir() {
            walk(&entry.path(), &format!("{path}/"), depth + 1, into)?;
        } else if metadata.is_file() {
            into.push(path);
        }
    }
    Ok(())
}

impl FileSource for DirSource {
    fn listing(&self) -> &Listing {
        &self.listing
    }

    fn read_listed(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.root.join(path))
    }

    fn origin(&self) -> &Path {
        &self.root
    }
}

/// The files of a zip archive, read from it as they are asked for: the
/// archive is never unpacked to the disk.
pub struct ZipSource {
    path: PathBuf,
    archive: Mutex<zip::ZipArchive<File>>,
    listing: Listing,
    /// Where each listed file is in the archive.
    indexes: HashMap<String, usize>,
}

impl ZipSource {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut archive = zip::ZipArchive::new(File::open(path)?).map_err(io::Error::other)?;
        let mut paths = Vec::new();
        let mut indexes = HashMap::new();
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).map_err(io::Error::other)?;
            if entry.is_dir() {
                continue;
            }
            // Only paths that stay inside the archive's top.
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            let name = name.to_string_lossy().replace('\\', "/");
            // What macOS's archiver adds beside each file.
            if name.starts_with("__MACOSX/") {
                continue;
            }
            indexes.insert(name.clone(), i);
            paths.push(name);
        }
        paths.sort();
        Ok(ZipSource {
            path: path.to_owned(),
            archive: Mutex::new(archive),
            listing: Listing::new(paths),
            indexes,
        })
    }
}

impl FileSource for ZipSource {
    fn listing(&self) -> &Listing {
        &self.listing
    }

    fn read_listed(&self, path: &str) -> io::Result<Vec<u8>> {
        let &index = self
            .indexes
            .get(path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_owned()))?;
        let mut archive = self.archive.lock().unwrap_or_else(PoisonError::into_inner);
        let mut entry = archive.by_index(index).map_err(io::Error::other)?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn origin(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_whatever_the_case() {
        let listing = Listing::new(vec!["Pack/Sounds/Kick.WAV".into(), "Pack/info".into()]);
        assert_eq!(
            listing.find("pack/sounds/kick.wav"),
            Some("Pack/Sounds/Kick.WAV")
        );
        assert_eq!(
            listing.find("./PACK\\SOUNDS\\KICK.wav"),
            Some("Pack/Sounds/Kick.WAV")
        );
        assert_eq!(listing.find("Pack/sounds/snare.wav"), None);
    }
}
