//! Covers read from their files, whatever their format, into a
//! [`Project`].

pub mod als;
pub mod source;
pub mod unipack;

use std::path::Path;
use std::sync::Arc;

use crate::model::Project;
pub use source::FileSource;

/// A cover read, with where its audio files are to be found.
pub struct Cover {
    pub project: Project,
    pub format: Format,
    /// The cover's files, which [`Project::samples`] are paths in.
    pub files: Arc<dyn FileSource>,
    /// What could not be read, for the user to know; the rest plays.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    UniPack,
    Ableton,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::UniPack => "UniPack",
            Format::Ableton => "Ableton Live Set",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a zip archive PadWire can read: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error(
        "no cover found here: a UniPack has an info and a keySound file, an Ableton cover a .als set"
    )]
    NotACover,
    #[error("{0}")]
    Invalid(String),
}

/// How covers are read.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Folders of Ableton's samples the user has, which Ableton covers may
    /// use without holding them: looked through as Ableton's libraries are
    /// where Live is installed.
    pub ableton_libraries: Vec<std::path::PathBuf>,
}

/// Read a cover: a UniPack, zipped or as a folder, or an Ableton Live Set,
/// or the folder of one.
pub fn open(path: &Path) -> Result<Cover, Error> {
    open_with(path, &Options::default())
}

pub fn open_with(path: &Path, options: &Options) -> Result<Cover, Error> {
    if als::is_set(path) {
        return als::read(path, options);
    }
    let files = source::open(path)?;
    if unipack::is_unipack(&*files) {
        return unipack::read(files);
    }
    if path.is_dir() {
        match als::sets_in(path).as_slice() {
            [] => {}
            [set] => return als::read(set, options),
            sets => {
                let names: Vec<String> = sets
                    .iter()
                    .filter_map(|set| set.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .collect();
                return Err(Error::Invalid(format!(
                    "this folder holds several sets; open the one to play: {}",
                    names.join(", ")
                )));
            }
        }
    }
    Err(Error::NotACover)
}

/// The text of a file, from UTF-8 or, with a byte order mark, UTF-16, as
/// Windows tools write it. Anything that is not text is replaced, so that a
/// title in another encoding still shows.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], big: bool| {
        let units = bytes.as_chunks::<2>().0.iter().map(|&pair| {
            if big {
                u16::from_be_bytes(pair)
            } else {
                u16::from_le_bytes(pair)
            }
        });
        char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    };
    match bytes {
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xfe, 0xff, rest @ ..] => utf16(rest, true),
        [0xff, 0xfe, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_with_byte_order_marks() {
        assert_eq!(decode_text(b"\xef\xbb\xbftitle=A"), "title=A");
        assert_eq!(decode_text(b"\xff\xfeA\0B\0"), "AB");
        assert_eq!(decode_text(b"\xfe\xff\0A\0B"), "AB");
        assert_eq!(decode_text(b"plain"), "plain");
    }
}
