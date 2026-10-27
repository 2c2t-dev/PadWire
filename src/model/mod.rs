//! What a cover is, whatever it was made with: pages of pads, each with the
//! sounds it plays and the lights it starts.
//!
//! Every format is turned into this, and the engine knows only this.

pub mod midifx;
pub mod palette;

use std::collections::HashMap;
use std::sync::Arc;

/// A cover, ready to be played.
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub title: String,
    pub author: Option<String>,
    /// Columns, then rows: 8 by 8 on every Launchpad so far.
    pub size: (u8, u8),
    /// The pages, "chains" in a UniPack, the Chain Selector in Ableton.
    pub pages: Vec<Page>,
    /// Every audio file the pads play, each once, by its place in the
    /// cover's files.
    pub samples: Vec<SampleFile>,
    /// Lights made by MIDI effects from every press, as Ableton covers make
    /// them, beside or in place of each pad's own.
    pub midi_lights: Option<Arc<midifx::MidiLights>>,
    /// The cover played through, as its maker plays it: to listen to, or to
    /// learn it by.
    pub autoplay: Option<Arc<AutoPlay>>,
}

/// A performance of a cover, press by press.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AutoPlay {
    /// In order of time.
    pub events: Vec<AutoEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoEvent {
    /// From the start.
    pub at_ms: f64,
    pub action: AutoAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAction {
    /// A pad pressed, by (x, y) from the bottom left.
    Press(u8, u8),
    Release(u8, u8),
    /// The page turned to. A UniPack's turns also start every pad's sounds
    /// over from their first.
    Page {
        page: usize,
        restart_sounds: bool,
    },
}

/// An audio file of the cover, or a part of one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SampleFile {
    /// Where it is among the cover's files, as the format's source reads it.
    pub path: String,
    /// The frames played, first and last, at the file's own rate, when it
    /// is not played whole.
    pub range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    /// By (x, y): columns and rows from 1, from the bottom left.
    pub pads: HashMap<(u8, u8), Pad>,
}

#[derive(Debug, Clone, Default)]
pub struct Pad {
    /// Played in turn: each press plays the next one, round and round.
    pub sounds: Vec<SoundRef>,
    /// Started in turn, as the sounds are, but counted apart from them.
    pub leds: Vec<LedProgram>,
    /// A press stops every sound of the same group still playing.
    pub choke_group: Option<u8>,
    pub playback: PlaybackMode,
}

/// One sound a pad plays.
///
/// The loop and the page to go to belong to each sound rather than to the
/// pad: in a UniPack, each line of a pad's queue has its own.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundRef {
    /// Its place in [`Project::samples`].
    pub sample: usize,
    pub loop_mode: LoopMode,
    /// The page to turn to once it has been pressed.
    pub wormhole: Option<usize>,
    pub gain: f32,
}

/// How many times something plays through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Once,
    /// So many times in all, the first one included.
    Times(u32),
    /// Until the pad is let go.
    Infinite,
}

impl LoopMode {
    /// The number of times through, or `None` for no end.
    pub fn passes(self) -> Option<u32> {
        match self {
            LoopMode::Once => Some(1),
            LoopMode::Times(n) => Some(n.max(1)),
            LoopMode::Infinite => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackMode {
    /// Plays to its end, whether the pad is held or not.
    #[default]
    OneShot,
    /// Fades out over `release_ms` as the pad is let go.
    Gate { release_ms: u32 },
}

#[derive(Debug, Clone)]
pub enum LedProgram {
    /// Lights at set times, from a UniPack or worked out ahead.
    Sequence(Arc<LedSequence>),
}

/// Lights at set times, played through as many times as its loop says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LedSequence {
    /// In order of time.
    pub events: Vec<LedEvent>,
    /// How long one time through lasts: past the last event when it ends
    /// on a delay.
    pub duration_ms: u32,
    pub loop_mode: LoopMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedEvent {
    /// From the start of the time through.
    pub at_ms: u32,
    pub action: LedAction,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LedAction {
    Set(LedTarget, LedColor),
    /// Turn to a page, which a UniPack's lights may do.
    Page(usize),
}

/// A light of the Launchpad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LedTarget {
    /// A pad of the grid, by (x, y) from 1, from the bottom left.
    Grid(u8, u8),
    /// A round button of the right side, from 1 at the bottom to 8 at the top.
    Side(u8),
    /// A round button of the top row, from 1 on the left to 8 on the right.
    Top(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LedColor {
    #[default]
    Off,
    /// A velocity, which the Launchpad shows as a colour of its palette.
    Palette(u8),
    /// A colour of its own. A UniPack often gives a velocity alongside, for
    /// the Launchpad, while the colour is what an app shows.
    Rgb { rgb: [u8; 3], palette: Option<u8> },
    /// A colour as the levels of the LED's red, green and blue, 0 to 63:
    /// a velocity of a cover's own palette.
    Levels([u8; 3]),
}

impl LedColor {
    pub fn is_off(self) -> bool {
        match self {
            LedColor::Off => true,
            LedColor::Palette(v) => v == 0,
            LedColor::Rgb { rgb, palette } => rgb == [0, 0, 0] && palette.unwrap_or(0) == 0,
            LedColor::Levels(levels) => levels == [0, 0, 0],
        }
    }

    /// How it looks on screen, or `None` when it is off.
    pub fn display_rgb(self) -> Option<[u8; 3]> {
        if self.is_off() {
            return None;
        }
        Some(match self {
            LedColor::Off => unreachable!(),
            LedColor::Palette(v) => palette::display_rgb(v),
            LedColor::Rgb { rgb, .. } => rgb,
            LedColor::Levels(levels) => levels.map(palette::level_to_display),
        })
    }
}

/// A pad of a page: what plays a sound and owns the lights it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PadKey {
    pub page: usize,
    pub x: u8,
    pub y: u8,
}
