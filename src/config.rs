//! What PadWire remembers between runs, in `~/.config/padwire/config.toml`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::formats::Format;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Frames the audio is played a buffer at a time.
    pub buffer_frames: u32,
    /// The MIDI port of the Launchpad, or any Launchpad found.
    pub midi_port: Option<String>,
    /// Whether the first page is the top button of the side, as in UniPad,
    /// or the bottom one.
    pub first_page_on_top: bool,
    /// The covers imported, the last opened first.
    pub library: Vec<LibraryEntry>,
    /// Folders of Ableton's samples, which Ableton covers may use without
    /// holding them: the user's own, from wherever Live is installed.
    pub ableton_libraries: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            buffer_frames: 256,
            midi_port: None,
            first_page_on_top: true,
            library: Vec::new(),
            ableton_libraries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub title: String,
    pub author: Option<String>,
    pub format: Format,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("padwire").join("config.toml"))
    }

    /// What was saved, or the defaults when nothing was or it cannot be read.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!(
                    "{} cannot be read, the defaults are used: {e}",
                    path.display()
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let saved = toml::to_string_pretty(self)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                // Written beside, then put in place, so that a crash
                // leaves the old file whole.
                let temporary = path.with_extension("toml.new");
                std::fs::write(&temporary, text).map_err(|e| e.to_string())?;
                std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
            });
        if let Err(e) = saved {
            log::warn!("{} cannot be saved: {e}", path.display());
        }
    }

    /// Put a cover first in the library, once.
    pub fn remember(&mut self, entry: LibraryEntry) {
        self.library.retain(|known| known.path != entry.path);
        self.library.insert(0, entry);
    }
}
