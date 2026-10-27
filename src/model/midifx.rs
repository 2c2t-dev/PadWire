//! Lights made by MIDI effects, as Ableton covers make them: the notes of
//! the pads go through chains of devices, and the notes that come out light
//! the Launchpad, the velocity as the colour.
//!
//! This is what the chains are; `engine::midifx` plays them.

/// The chains that make a cover's lights, each fed every note of the pads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MidiLights {
    /// One a track of lights.
    pub chains: Vec<Chain>,
    /// Beats a minute, which synced devices keep time with.
    pub tempo: f32,
    /// The note of each page's button, from the first page's: what the
    /// buttons send, and what lights them.
    pub page_notes: u8,
    /// The colours the cover's velocities stand for, as LED levels of 0 to
    /// 63, when it says which: a velocity each, from 0.
    pub palette: Option<std::sync::Arc<Vec<[u8; 3]>>>,
}

pub type Chain = Vec<Device>;

#[derive(Debug, Clone, PartialEq)]
pub enum Device {
    /// Every note moved by `shift` semitones; notes outside
    /// `lowest..=lowest + range` pass unchanged.
    Pitch {
        shift: i32,
        lowest: u8,
        range: u8,
    },
    /// Each note with others at these intervals, at these parts of its
    /// velocity.
    Chord {
        shifts: Vec<(i32, f32)>,
    },
    Velocity(VelocityMap),
    /// Every note made this long, from its start or, `on_release`, from its
    /// end.
    NoteLength {
        ms: f32,
        on_release: bool,
    },
    Arpeggiator(Arpeggiator),
    Rack(Rack),
    /// Lights recorded in a MIDI file, played from its start at every note
    /// that comes in, as Kaskobi's MIDI Launcher does; the notes that come
    /// in go no further.
    MidiFile(std::sync::Arc<MidiFile>),
    /// Lights that move away from the pad pressed, as Kaskobi's Resonator
    /// makes them; the note pressed goes no further.
    Resonator(std::sync::Arc<Resonator>),
    /// Puts the notes in a layer, as Depths Selector does: a light shows
    /// the colour of the highest layer lit on it, as Apollo Studio, by the
    /// same author, lays layers out.
    Layer(i8),
}

/// Lines and shapes that leave the pad pressed, each light going through
/// a gradient of colours, as Kaskobi documents his Resonator: written anew
/// from its description and the settings a cover keeps, not from its code.
#[derive(Debug, Clone, PartialEq)]
pub struct Resonator {
    /// The directions lines go in, a pad at a time: (x, y), up positive.
    pub directions: Vec<(i8, i8)>,
    /// The shapes that grow round the pad.
    pub shapes: Vec<Shape>,
    /// How far they go, in pads.
    pub steps: u8,
    /// The time between one pad and the next.
    pub move_ms: f32,
    /// From the far end towards the pad rather than away from it.
    pub inwards: bool,
    /// The velocities each light goes through; `None` keeps the velocity
    /// of the note pressed.
    pub gradient: Option<Vec<u8>>,
    /// The time between one colour and the next.
    pub colour_ms: f32,
    /// How long a light lasts when its gradient loops, or when it has none.
    pub length_ms: f32,
    /// The gradient played over and over while the light lasts.
    pub loop_gradient: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Square,
    Circle,
    Diamond,
}

impl Shape {
    /// Whether a pad `(dx, dy)` away is on the ring `distance` pads out.
    pub fn on_ring(self, dx: i32, dy: i32, distance: i32) -> bool {
        match self {
            Shape::Square => dx.abs().max(dy.abs()) == distance,
            Shape::Diamond => dx.abs() + dy.abs() == distance,
            Shape::Circle => (f64::from(dx * dx + dy * dy).sqrt().round() as i32) == distance,
        }
    }
}

/// Live's synced rates as Kaskobi's devices list them, in beats.
pub fn max_rate_beats(index: u8) -> f32 {
    const RATES: [f32; 11] = [
        1.0 / 128.0,
        1.0 / 64.0,
        1.0 / 48.0,
        1.0 / 32.0,
        1.0 / 24.0,
        1.0 / 16.0,
        1.0 / 12.0,
        1.0 / 8.0,
        1.0 / 4.0,
        1.0 / 2.0,
        1.0,
    ];
    RATES[usize::from(index).min(RATES.len() - 1)] * 4.0
}

/// The notes of a MIDI file, in time.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MidiFile {
    /// In order of time, which is counted in beats from the start.
    pub notes: Vec<FileNote>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileNote {
    pub beat: f64,
    pub note: u8,
    /// 0 for the end of a note.
    pub velocity: u8,
}

/// Velocities mapped from an input range to an output one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VelocityMap {
    pub lowest: u8,
    pub range: u8,
    pub out_low: u8,
    pub out_high: u8,
    pub mode: VelocityMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityMode {
    /// Velocities outside the input range are taken as its nearest end.
    Clip,
    /// Notes of velocities outside the input range are dropped.
    Gate,
    /// Every note at the highest output velocity.
    Fixed,
}

impl VelocityMap {
    /// A note's velocity, or `None` when the note is dropped.
    pub fn map(&self, velocity: u8) -> Option<u8> {
        let low = f32::from(self.lowest);
        let high = (f32::from(self.lowest) + f32::from(self.range)).min(127.0);
        let velocity = f32::from(velocity);
        let input = match self.mode {
            VelocityMode::Fixed => return Some(self.out_high.max(1)),
            VelocityMode::Gate if velocity < low || velocity > high => return None,
            _ => velocity.clamp(low, high),
        };
        let part = if high > low {
            (input - low) / (high - low)
        } else {
            1.0
        };
        let out =
            f32::from(self.out_low) + part * (f32::from(self.out_high) - f32::from(self.out_low));
        Some(out.round().clamp(1.0, 127.0) as u8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arpeggiator {
    pub style: ArpStyle,
    /// The time between two notes.
    pub step_ms: f32,
    /// How long each note lasts, as a part of a step: 2.0 for 200 %.
    pub gate: f32,
    /// How many times the pattern plays, or `None` for no end.
    pub repeats: Option<u32>,
    /// Whether it goes on once the notes are let go.
    pub hold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpStyle {
    Up,
    Down,
    UpDown,
    DownUp,
}

impl ArpStyle {
    /// The order the notes are played in, as places in them sorted up.
    pub fn order(self, count: usize) -> Vec<usize> {
        let up: Vec<usize> = (0..count).collect();
        let down: Vec<usize> = (0..count).rev().collect();
        // Back the other way, without playing the ends twice.
        let back = |there: &[usize]| -> Vec<usize> {
            let len = there.len();
            if len < 3 {
                return Vec::new();
            }
            there[1..len - 1].iter().rev().copied().collect()
        };
        match self {
            ArpStyle::Up => up,
            ArpStyle::Down => down,
            ArpStyle::UpDown => {
                let mut order = up.clone();
                order.extend(back(&up));
                order
            }
            ArpStyle::DownUp => {
                let mut order = down.clone();
                order.extend(back(&down));
                order
            }
        }
    }
}

/// A rack: each note goes to the branches whose zones it falls in.
#[derive(Debug, Clone, PartialEq)]
pub struct Rack {
    pub selector: Selector,
    pub branches: Vec<Branch>,
}

/// What a rack's Chain Selector stands at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Selector {
    Fixed(f32),
    /// Set to the page's number, from 0, by a device that switches pages.
    PageNumber,
    /// Mapped to the page buttons: `first` on the first page's note, `last`
    /// on the note `pages - 1` buttons on.
    Page {
        first: f32,
        last: f32,
        pages: u8,
    },
}

impl Selector {
    pub fn value(&self, page: usize) -> f32 {
        match *self {
            Selector::Fixed(value) => value,
            Selector::PageNumber => page as f32,
            Selector::Page { first, last, pages } => {
                let span = f32::from(pages.max(2) - 1);
                let page = (page as f32).min(span);
                (first + (last - first) * page / span).round()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    pub keys: (u8, u8),
    pub velocities: (u8, u8),
    pub selector: (f32, f32),
    pub chain: Chain,
}

impl Branch {
    pub fn takes(&self, note: u8, velocity: u8, selector: f32) -> bool {
        (self.keys.0..=self.keys.1).contains(&note)
            && (velocity == 0 || (self.velocities.0..=self.velocities.1).contains(&velocity))
            && (self.selector.0..=self.selector.1).contains(&selector)
    }
}

/// How fast a synced arpeggiator goes, by Live's index of its rates: from
/// a 128th note to a whole one, each straight one followed by its triplet.
/// Live's presets have 16th at 6, 12th at 7 and 8th at 8.
pub fn synced_rate_beats(index: u8) -> f32 {
    const RATES: [f32; 14] = [
        1.0 / 128.0,
        1.0 / 96.0,
        1.0 / 64.0,
        1.0 / 48.0,
        1.0 / 32.0,
        1.0 / 24.0,
        1.0 / 16.0,
        1.0 / 12.0,
        1.0 / 8.0,
        1.0 / 6.0,
        1.0 / 4.0,
        1.0 / 3.0,
        1.0 / 2.0,
        1.0,
    ];
    // In beats: a quarter note is one.
    RATES[usize::from(index).min(RATES.len() - 1)] * 4.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_to_the_top_of_its_range() {
        // As the Faded cover sets it: 127 from the Launchpad gives 94.
        let map = VelocityMap {
            lowest: 1,
            range: 127,
            out_low: 1,
            out_high: 94,
            mode: VelocityMode::Clip,
        };
        assert_eq!(map.map(127), Some(94));
        assert_eq!(map.map(1), Some(1));
        let gate = VelocityMap {
            lowest: 64,
            range: 63,
            out_low: 1,
            out_high: 127,
            mode: VelocityMode::Gate,
        };
        assert_eq!(gate.map(10), None);
    }

    #[test]
    fn arpeggio_orders() {
        assert_eq!(ArpStyle::Up.order(3), [0, 1, 2]);
        assert_eq!(ArpStyle::Down.order(3), [2, 1, 0]);
        assert_eq!(ArpStyle::UpDown.order(4), [0, 1, 2, 3, 2, 1]);
        assert_eq!(ArpStyle::DownUp.order(3), [2, 1, 0, 1]);
        assert_eq!(ArpStyle::UpDown.order(1), [0]);
    }

    #[test]
    fn selector_over_pages() {
        let selector = Selector::Page {
            first: 0.0,
            last: 7.0,
            pages: 8,
        };
        assert_eq!(selector.value(0), 0.0);
        assert_eq!(selector.value(5), 5.0);
        assert_eq!(synced_rate_beats(6), 0.25);
        assert_eq!(synced_rate_beats(8), 0.5);
    }
}
