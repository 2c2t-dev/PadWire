//! Where a pad is in the notes of each layout a Launchpad has, and the way
//! from one layout to the other.
//!
//! Pads are (x, y), columns and rows from 1, from the bottom left, as in
//! the model.

/// The Programmer layout of the Mini MK3, the X and the Pro MK3: the row
/// as tens, the column as units, 11 at the bottom left to 88 at the top
/// right.
pub mod programmer {
    pub fn note(x: u8, y: u8) -> u8 {
        y * 10 + x
    }

    pub fn pad(note: u8) -> Option<(u8, u8)> {
        let (y, x) = (note / 10, note % 10);
        ((1..=8).contains(&x) && (1..=8).contains(&y)).then_some((x, y))
    }

    /// The controller number of a round button of the right side, 1 at the
    /// bottom: 19 to 89.
    pub fn side_cc(n: u8) -> u8 {
        n * 10 + 9
    }

    pub fn side(cc: u8) -> Option<u8> {
        (cc % 10 == 9 && (19..=89).contains(&cc)).then_some(cc / 10)
    }

    /// The controller number of a round button of the top row, 1 on the
    /// left: 91 to 98.
    pub fn top_cc(n: u8) -> u8 {
        90 + n
    }

    pub fn top(cc: u8) -> Option<u8> {
        (91..=98).contains(&cc).then_some(cc - 90)
    }
}

/// The Drum Rack layout of the MK2 and the Pro in User mode, which most
/// older Ableton covers are made for: two halves of four columns, each
/// counting up four notes a row, the left half from 36 and the right from
/// 68.
pub mod drum_rack {
    /// The MIDI channel, counted from 0: channel 6 as Ableton shows it.
    pub const CHANNEL: u8 = 5;

    pub fn note(x: u8, y: u8) -> u8 {
        let (column, row) = (x - 1, y - 1);
        let half = column / 4;
        36 + half * 32 + row * 4 + column % 4
    }

    pub fn pad(note: u8) -> Option<(u8, u8)> {
        if !(36..=99).contains(&note) {
            return None;
        }
        let (half, j) = ((note - 36) / 32, (note - 36) % 32);
        let (row, column) = (j / 4, j % 4 + 4 * half);
        Some((column + 1, row + 1))
    }

    /// The note of a round button of the right side, 1 at the bottom:
    /// 100 to 107. Which end 100 is at is still to be checked.
    pub fn side_note(n: u8) -> u8 {
        99 + n
    }

    pub fn side(note: u8) -> Option<u8> {
        (100..=107).contains(&note).then_some(note - 99)
    }
}

/// A Programmer note as the Drum Rack note of the same pad.
pub fn programmer_to_drum_rack(note: u8) -> Option<u8> {
    programmer::pad(note).map(|(x, y)| drum_rack::note(x, y))
}

/// A Drum Rack note as the Programmer note of the same pad.
pub fn drum_rack_to_programmer(note: u8) -> Option<u8> {
    drum_rack::pad(note).map(|(x, y)| programmer::note(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pads() -> impl Iterator<Item = (u8, u8)> {
        (1..=8).flat_map(|y| (1..=8).map(move |x| (x, y)))
    }

    #[test]
    fn programmer_round_trip() {
        for (x, y) in pads() {
            assert_eq!(programmer::pad(programmer::note(x, y)), Some((x, y)));
        }
        assert_eq!(programmer::note(1, 1), 11);
        assert_eq!(programmer::note(8, 8), 88);
        assert_eq!(programmer::pad(19), None);
        assert_eq!(programmer::pad(90), None);
    }

    #[test]
    fn drum_rack_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for (x, y) in pads() {
            let note = drum_rack::note(x, y);
            assert!((36..=99).contains(&note));
            assert!(seen.insert(note), "two pads on note {note}");
            assert_eq!(drum_rack::pad(note), Some((x, y)));
        }
        // The corners, and the first pad of the right half.
        assert_eq!(drum_rack::note(1, 1), 36);
        assert_eq!(drum_rack::note(4, 8), 67);
        assert_eq!(drum_rack::note(5, 1), 68);
        assert_eq!(drum_rack::note(8, 8), 99);
    }

    /// Both ways, on all 64 pads, as the prototype bridge checked them.
    #[test]
    fn between_layouts() {
        for (x, y) in pads() {
            let note = programmer::note(x, y);
            let drum = programmer_to_drum_rack(note).unwrap();
            assert_eq!(drum_rack_to_programmer(drum), Some(note));
        }
        // The column the Faded cover lights going up: 47, 51, 55 and on.
        assert_eq!(drum_rack::pad(47), Some((4, 3)));
        assert_eq!(drum_rack::pad(51), Some((4, 4)));
    }

    #[test]
    fn round_buttons() {
        for n in 1..=8 {
            assert_eq!(programmer::side(programmer::side_cc(n)), Some(n));
            assert_eq!(programmer::top(programmer::top_cc(n)), Some(n));
            assert_eq!(drum_rack::side(drum_rack::side_note(n)), Some(n));
        }
        assert_eq!(programmer::side_cc(1), 19);
        assert_eq!(programmer::side_cc(8), 89);
    }
}
