//! Where a set's samples are, now that it is on another computer than the
//! one it was made on.
//!
//! A set says where each sample was: relative to the set, and as the full
//! path on its maker's computer. The relative path is tried first. If it
//! leads nowhere, the sample is looked for among the cover's files, by the
//! longest end of its old path found there (`Faded/Page 3/56.wav`, then
//! `Page 3/56.wav`), and by its name alone only when no other file has it:
//! covers have a `1.wav` in every page's folder. The size the set
//! remembers tells apart files that are still alike. This is how the
//! prototype `fix_als_paths.py` repaired the 561 paths of a real cover.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use super::xml::Node;

/// The kinds of drum sounds, by the words their names have.
const KINDS: &[&[&str]] = &[
    &["kick", "bassdrum", "bass drum"],
    &["snare"],
    &["clap"],
    &["hihat", "hi-hat", "hi hat", "hat"],
    &["crash"],
    &["ride"],
    &["tom"],
    &["rim"],
    &["shaker"],
    &["cymbal"],
];

/// The files taken for sounds.
const AUDIO: &[&str] = &["wav", "aif", "aiff", "flac", "mp3", "ogg"];

/// How many files are looked at, at most, against a cover put in a large
/// folder by mistake.
const MAX_FILES: usize = 100_000;
const MAX_DEPTH: usize = 8;

/// What a set says of where a sample was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileRef {
    /// From the set's folder, `..` for a folder up, the name last.
    pub relative: Vec<String>,
    /// The folders it was in on the computer the set was made on, the name
    /// last.
    pub original: Vec<String>,
    /// Its size in bytes, as the set remembers it.
    pub size: Option<u64>,
}

impl FileRef {
    /// From a `<FileRef>`, of Live 9 and before or of Live 10 and after.
    pub fn read(node: &Node) -> Option<FileRef> {
        let mut file = FileRef::default();
        match node.child("RelativePath") {
            // Live 10 and after: paths as text.
            Some(relative) if relative.value().is_some() => {
                file.relative = split(relative.value().unwrap_or_default());
                file.original = split(node.value_at("Path").unwrap_or_default());
                file.size = node.parse_at("OriginalFileSize");
            }
            // Live 9: a folder an element, an empty one for a folder up,
            // and the name apart.
            relative => {
                let name = node.value_at("Name")?.to_owned();
                if let Some(relative) = relative {
                    file.relative = relative
                        .children("RelativePathElement")
                        .map(|element| match element.attr("Dir") {
                            Some("") | None => "..".to_owned(),
                            Some(dir) => dir.to_owned(),
                        })
                        .collect();
                    file.relative.push(name.clone());
                }
                if let Some(hint) = node.at("SearchHint/PathHint") {
                    file.original = hint
                        .children("RelativePathElement")
                        .filter_map(|element| element.attr("Dir").filter(|dir| !dir.is_empty()))
                        .map(str::to_owned)
                        .collect();
                    file.original.push(name);
                }
                file.size = node.parse_at("SearchHint/FileSize");
            }
        }
        (!file.relative.is_empty() || !file.original.is_empty()).then_some(file)
    }

    pub fn name(&self) -> Option<&str> {
        self.original
            .last()
            .or(self.relative.last())
            .map(String::as_str)
    }
}

/// A path's parts, whichever way its slashes go, with the drive of a
/// Windows path left out.
fn split(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty() && !part.ends_with(':'))
        .map(str::to_owned)
        .collect()
}

/// Files to look samples up among: the cover's, or Ableton's library.
pub struct Resolver {
    files: Vec<PathBuf>,
    /// Each file's place, by its name as compared.
    by_name: HashMap<String, Vec<usize>>,
    /// Whether names are compared without their dashes and underscores, as
    /// Ableton's library renamed its samples from one Live to the next:
    /// Live 9's `Kick-LD.aif` is Live 12's `Kick LD.aif`.
    loose: bool,
}

impl Resolver {
    /// Every file under `root`.
    pub fn new(root: &Path) -> Self {
        Self::under(&[root.to_owned()], false)
    }

    /// Every sample of Ableton's libraries found on this computer, and of
    /// the folders of Ableton's the user added.
    pub fn libraries(added: &[PathBuf]) -> Self {
        let mut roots = ableton_libraries();
        roots.extend(added.iter().filter(|path| path.is_dir()).cloned());
        Self::under(&roots, true)
    }

    fn under(roots: &[PathBuf], loose: bool) -> Self {
        let mut files = Vec::new();
        for root in roots {
            walk(root, 0, &mut files);
        }
        files.sort();
        let mut resolver = Resolver {
            files,
            by_name: HashMap::new(),
            loose,
        };
        for i in 0..resolver.files.len() {
            if let Some(name) = resolver.files[i].file_name() {
                let key = resolver.key(&name.to_string_lossy());
                resolver.by_name.entry(key).or_default().push(i);
            }
        }
        resolver
    }

    /// A name or a folder as it is compared.
    fn key(&self, part: &str) -> String {
        let lower = part.to_lowercase();
        if self.loose {
            lower.replace(['-', '_'], " ")
        } else {
            lower
        }
    }

    /// Where the sample is, from the folders its relative path may start
    /// in, or `None` when it cannot be told.
    pub fn resolve(&self, file: &FileRef, bases: &[PathBuf]) -> Option<PathBuf> {
        if !file.relative.is_empty() {
            for base in bases {
                let path = lexical_join(base, &file.relative);
                if let Some(found) = self.exactly(&path) {
                    return Some(found);
                }
            }
        }
        let name = self.key(file.name()?);
        let Some(candidates) = self.by_name.get(&name) else {
            return self.renamed(file);
        };
        for parts in [&file.original, &file.relative] {
            let parts: Vec<String> = parts
                .iter()
                .filter(|p| *p != "..")
                .map(|p| self.key(p))
                .collect();
            for k in (2..=parts.len()).rev() {
                let tail = &parts[parts.len() - k..];
                let hits: Vec<usize> = candidates
                    .iter()
                    .copied()
                    .filter(|&i| self.ends_with(&self.files[i], tail))
                    .collect();
                if let Some(found) = self.one_of(&hits, file.size) {
                    return Some(found);
                }
            }
        }
        self.one_of(candidates, file.size)
    }

    /// A sample of Ableton's library renamed beyond recognition, as
    /// `Snare-909-Tune8-s.wav` became `Snare 909 Tune 2.wav`: the one file of
    /// the same kind and the same size, or of those, the one in a folder of
    /// the same name. Only the library is looked through this way; a
    /// cover's own files are never taken for one another.
    fn renamed(&self, file: &FileRef) -> Option<PathBuf> {
        let size = file.size?;
        if !self.loose {
            return None;
        }
        let kind = |path: &Path| path.extension().map(|e| e.to_string_lossy().to_lowercase());
        let wanted = kind(Path::new(file.name()?));
        let alike: Vec<&PathBuf> = self
            .files
            .iter()
            .filter(|path| kind(path) == wanted)
            .filter(|path| std::fs::metadata(path).is_ok_and(|m| m.len() == size))
            .collect();
        let folder = |parts: &[String]| parts.iter().rev().nth(1).map(|dir| self.key(dir));
        let wanted_folder = folder(&file.original).or_else(|| folder(&file.relative));
        match alike[..] {
            [only] => Some(only.clone()),
            _ => {
                let in_folder: Vec<&&PathBuf> = alike
                    .iter()
                    .filter(|path| {
                        let parent = path
                            .parent()
                            .and_then(Path::file_name)
                            .map(|d| self.key(&d.to_string_lossy()));
                        parent.is_some() && parent == wanted_folder
                    })
                    .collect();
                match in_folder[..] {
                    [only] => Some((*only).clone()),
                    _ => None,
                }
            }
        }
    }

    /// The file at this path, whatever the case of its name.
    fn exactly(&self, path: &Path) -> Option<PathBuf> {
        if path.is_file() {
            return Some(path.to_owned());
        }
        let name = self.key(&path.file_name()?.to_string_lossy());
        let wanted = path.to_string_lossy().to_lowercase();
        self.by_name
            .get(&name)?
            .iter()
            .map(|&i| &self.files[i])
            .find(|file| file.to_string_lossy().to_lowercase() == wanted)
            .cloned()
    }

    /// The one file of these, or the one of the size the set remembers.
    fn one_of(&self, hits: &[usize], size: Option<u64>) -> Option<PathBuf> {
        match hits {
            [] => None,
            [only] => Some(self.files[*only].clone()),
            _ => {
                let size = size?;
                let sized: Vec<usize> = hits
                    .iter()
                    .copied()
                    .filter(|&i| std::fs::metadata(&self.files[i]).is_ok_and(|m| m.len() == size))
                    .collect();
                match sized[..] {
                    [only] => Some(self.files[only].clone()),
                    _ => None,
                }
            }
        }
    }

    /// A sound of the same kind as a missing one, by the kind its name says:
    /// a kick for a kick. Of several, the first by path.
    pub fn alike(&self, name: &str) -> Option<PathBuf> {
        let name = name.to_lowercase();
        let kind = KINDS
            .iter()
            .find(|kinds| kinds.iter().any(|word| name.contains(word)))?;
        self.files
            .iter()
            .filter(|file| {
                let audio = file.extension().is_some_and(|extension| {
                    AUDIO.contains(&extension.to_string_lossy().to_lowercase().as_str())
                });
                let file_name = file.file_name().map(|n| n.to_string_lossy().to_lowercase());
                audio && file_name.is_some_and(|n| kind.iter().any(|word| n.contains(word)))
            })
            .min()
            .cloned()
    }

    fn ends_with(&self, path: &Path, tail: &[String]) -> bool {
        let parts: Vec<String> = path
            .components()
            .filter_map(|c| match c {
                Component::Normal(part) => Some(self.key(&part.to_string_lossy())),
                _ => None,
            })
            .collect();
        parts.len() >= tail.len() && parts[parts.len() - tail.len()..] == *tail
    }
}

/// Ableton's libraries wherever Live is installed under Wine, as plain
/// Wine, Bottles or Lutris install it: its Core Library, and the user's own
/// library and packs. Covers use their samples, which their makers had and
/// did not pack.
pub fn ableton_libraries() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut prefixes: Vec<PathBuf> = std::env::var_os("WINEPREFIX")
        .filter(|prefix| !prefix.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .collect();
    let children = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    };
    if let Some(home) = &home {
        // Wine's own prefixes.
        prefixes.extend(children(home).into_iter().filter(|path| {
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".wine"))
        }));
        // Bottles, as a Flatpak or not, and Lutris's games.
        for bottles in [
            ".var/app/com.usebottles.bottles/data/bottles/bottles",
            ".local/share/bottles/bottles",
            "Games",
        ] {
            prefixes.extend(children(&home.join(bottles)));
        }
    }
    let mut libraries = Vec::new();
    let mut add = |library: PathBuf| {
        if library.is_dir() && !libraries.contains(&library) {
            libraries.push(library);
        }
    };
    for prefix in prefixes {
        let drive = prefix.join("drive_c");
        for live in children(&drive.join("ProgramData/Ableton")) {
            add(live.join("Resources/Core Library"));
        }
        for user in children(&drive.join("users")) {
            add(user.join("Documents/Ableton/User Library"));
            add(user.join("Documents/Ableton/Factory Packs"));
        }
    }
    libraries.sort();
    libraries
}

/// `base` followed by `parts`, `..` going up a folder, without asking the
/// disk.
fn lexical_join(base: &Path, parts: &[String]) -> PathBuf {
    let mut path = base.to_owned();
    for part in parts {
        if part == ".." {
            path.pop();
        } else if part != "." {
            path.push(part);
        }
    }
    path
}

fn walk(dir: &Path, depth: usize, into: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || into.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            walk(&path, depth + 1, into);
        } else if metadata.is_file() && !name.ends_with(".asd") && into.len() < MAX_FILES {
            // `.asd`, Live's analysis of a sample, is never one.
            into.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::als::xml::parse;

    #[test]
    fn live_9_references() {
        let xml = r#"<FileRef>
            <HasRelativePath Value="true" /><RelativePathType Value="1" />
            <RelativePath><RelativePathElement Dir="" /><RelativePathElement Dir="Page 1" /></RelativePath>
            <Name Value="2.wav" />
            <SearchHint><PathHint>
                <RelativePathElement Dir="Users" /><RelativePathElement Dir="Faded" />
                <RelativePathElement Dir="Page 1" />
            </PathHint><FileSize Value="121466" /></SearchHint>
        </FileRef>"#;
        let file = FileRef::read(&parse(xml.as_bytes(), |_, _| false).unwrap()).unwrap();
        assert_eq!(file.relative, ["..", "Page 1", "2.wav"]);
        assert_eq!(file.original, ["Users", "Faded", "Page 1", "2.wav"]);
        assert_eq!(file.size, Some(121466));
    }

    #[test]
    fn live_12_references() {
        let xml = r#"<FileRef>
            <RelativePathType Value="0" /><RelativePath Value="" />
            <Path Value="C:\Users\Hajime\Faded\Page 1\2.wav" />
            <OriginalFileSize Value="121466" />
        </FileRef>"#;
        let file = FileRef::read(&parse(xml.as_bytes(), |_, _| false).unwrap()).unwrap();
        assert!(file.relative.is_empty());
        assert_eq!(
            file.original,
            ["Users", "Hajime", "Faded", "Page 1", "2.wav"]
        );
        assert_eq!(file.name(), Some("2.wav"));
    }

    #[test]
    fn found_by_the_longest_end_of_the_old_path() {
        let root = std::env::temp_dir().join(format!("padwire-paths-{}", std::process::id()));
        for (dir, size) in [("Page 1", 3), ("Page 2", 5)] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("1.wav"), vec![0u8; size]).unwrap();
        }
        std::fs::create_dir_all(root.join("Open Here")).unwrap();
        std::fs::write(root.join("Page 2").join("Only.wav"), b"x").unwrap();
        let resolver = Resolver::new(&root);
        let bases = [root.join("Open Here")];
        let old = |parts: &[&str], size| FileRef {
            relative: Vec::new(),
            original: parts.iter().map(|p| p.to_string()).collect(),
            size,
        };

        // The folder tells.
        let found = resolver.resolve(
            &old(&["C:", "Me", "Cover", "page 2", "1.wav"], None),
            &bases,
        );
        assert_eq!(found, Some(root.join("Page 2/1.wav")));
        // A name found once.
        let found = resolver.resolve(&old(&["Elsewhere", "only.WAV"], None), &bases);
        assert_eq!(found, Some(root.join("Page 2/Only.wav")));
        // A name found twice, in no folder that tells: only the size can.
        assert_eq!(
            resolver.resolve(&old(&["Other", "1.wav"], None), &bases),
            None
        );
        let found = resolver.resolve(&old(&["Other", "1.wav"], Some(5)), &bases);
        assert_eq!(found, Some(root.join("Page 2/1.wav")));
        // The relative path, up a folder from the set's.
        let relative = FileRef {
            relative: vec!["..".into(), "PAGE 1".into(), "1.wav".into()],
            ..FileRef::default()
        };
        assert_eq!(
            resolver.resolve(&relative, &bases),
            Some(root.join("Page 1/1.wav"))
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_sound_of_the_same_kind() {
        let root = std::env::temp_dir().join(format!("padwire-alike-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Page 2")).unwrap();
        for name in ["kick.wav", "snare.wav", "notes.txt", "1.wav"] {
            std::fs::write(root.join("Page 2").join(name), b"x").unwrap();
        }
        let resolver = Resolver::new(&root);
        assert_eq!(
            resolver.alike("Kick-LD.aif"),
            Some(root.join("Page 2/kick.wav"))
        );
        assert_eq!(
            resolver.alike("Snare-909-Tune8-s.wav"),
            Some(root.join("Page 2/snare.wav"))
        );
        // Nothing of the kind, or no kind said.
        assert_eq!(resolver.alike("Clap 808.wav"), None);
        assert_eq!(resolver.alike("56.wav"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn library_samples_renamed() {
        let root = std::env::temp_dir().join(format!("padwire-library-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Samples/One Shots/Drums/Kick")).unwrap();
        std::fs::write(root.join("Samples/One Shots/Drums/Kick/Kick LD.aif"), b"x").unwrap();
        let library = Resolver::under(std::slice::from_ref(&root), true);
        // As a Live 9 set names it in the library of its day.
        let old = FileRef {
            relative: vec![
                "Samples".into(),
                "Drums".into(),
                "Kick".into(),
                "Kick-LD.aif".into(),
            ],
            ..FileRef::default()
        };
        assert_eq!(
            library.resolve(&old, &[]),
            Some(root.join("Samples/One Shots/Drums/Kick/Kick LD.aif"))
        );
        // The cover's own files are compared strictly.
        assert_eq!(
            Resolver::under(std::slice::from_ref(&root), false).resolve(&old, &[]),
            None
        );
        // Renamed past recognising, the same size: in the folder of the
        // same name, not the other one.
        std::fs::create_dir_all(root.join("Samples/One Shots/Drums/Snare")).unwrap();
        std::fs::create_dir_all(root.join("Samples/One Shots/Drums/Clap")).unwrap();
        std::fs::write(
            root.join("Samples/One Shots/Drums/Snare/Snare 909 Tune 2.wav"),
            [0u8; 7],
        )
        .unwrap();
        std::fs::write(
            root.join("Samples/One Shots/Drums/Clap/Clap 2.wav"),
            [0u8; 7],
        )
        .unwrap();
        let library = Resolver::under(std::slice::from_ref(&root), true);
        let old = FileRef {
            original: vec![
                "Core Library".into(),
                "Samples".into(),
                "Drums".into(),
                "Snare".into(),
                "Snare-909-Tune8-s.wav".into(),
            ],
            size: Some(7),
            ..FileRef::default()
        };
        assert_eq!(
            library.resolve(&old, &[]),
            Some(root.join("Samples/One Shots/Drums/Snare/Snare 909 Tune 2.wav"))
        );
        let without_size = FileRef { size: None, ..old };
        assert_eq!(library.resolve(&without_size, &[]), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
