//! The Launchpad Mini MK3, driven in Programmer mode through its MIDI port.
//!
//! It has two ports: `LPMiniMK3 DA`, for a DAW, and `LPMiniMK3 MI`, which
//! Programmer mode goes through. UniPad's driver does the same: it leaves
//! DAW mode and enters Programmer mode, both on the MIDI port.

use super::{DeviceInput, Profile};
use crate::device::layout::programmer;
use crate::model::{LedColor, LedTarget};

/// The start of every SysEx message to it: Novation, then the Mini MK3.
const HEADER: [u8; 6] = [0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d];

pub struct MiniMk3;

impl MiniMk3 {
    fn sysex(body: &[u8]) -> Vec<u8> {
        let mut message = HEADER.to_vec();
        message.extend_from_slice(body);
        message.push(0xf7);
        message
    }

    /// The number a light goes by in its messages: its note on the grid,
    /// its controller number for a round button.
    fn index(target: LedTarget) -> Option<(u8, bool)> {
        match target {
            LedTarget::Grid(x, y) if (1..=8).contains(&x) && (1..=8).contains(&y) => {
                Some((programmer::note(x, y), false))
            }
            LedTarget::Side(n) if (1..=8).contains(&n) => Some((programmer::side_cc(n), true)),
            LedTarget::Top(n) if (1..=8).contains(&n) => Some((programmer::top_cc(n), true)),
            _ => None,
        }
    }
}

impl Profile for MiniMk3 {
    fn name(&self) -> &'static str {
        "Launchpad Mini MK3"
    }

    fn matches_port(&self, port: &str) -> bool {
        // ALSA names it `Launchpad Mini MK3 LPMiniMK3 MI`; the generic USB
        // driver may give `Launchpad Mini MK3 MIDI 2` instead, still to be
        // checked.
        port.contains("LPMiniMK3 MI")
            || (port.contains("Launchpad Mini MK3") && port.contains("MIDI 2"))
    }

    fn init_messages(&self) -> Vec<Vec<u8>> {
        vec![
            // Out of DAW mode, in case a DAW left it there.
            Self::sysex(&[0x10, 0x00]),
            // Into Programmer mode.
            Self::sysex(&[0x0e, 0x01]),
        ]
    }

    fn exit_messages(&self) -> Vec<Vec<u8>> {
        // Back to Live mode, where it lights its own layouts again.
        vec![Self::sysex(&[0x0e, 0x00])]
    }

    fn decode(&self, message: &[u8]) -> Option<DeviceInput> {
        let [status, data1, data2] = *message else {
            return None;
        };
        // Channel 1 only: the other channels carry nothing in Programmer mode.
        match status {
            0x90 | 0x80 => {
                let (x, y) = programmer::pad(data1)?;
                let velocity = if status == 0x80 { 0 } else { data2 };
                Some(DeviceInput::Pad { x, y, velocity })
            }
            0xb0 => {
                let pressed = data2 > 0;
                if let Some(n) = programmer::side(data1) {
                    Some(DeviceInput::Side { n, pressed })
                } else {
                    programmer::top(data1).map(|n| DeviceInput::Top { n, pressed })
                }
            }
            _ => None,
        }
    }

    fn led_message(&self, target: LedTarget, color: LedColor, out: &mut Vec<u8>) {
        out.clear();
        let Some((index, round)) = Self::index(target) else {
            return;
        };
        // A note on channel 1 for the grid, a controller for the round
        // buttons, with the velocity as the colour of the palette.
        let mut palette = |velocity: u8| {
            out.extend_from_slice(&[if round { 0xb0 } else { 0x90 }, index, velocity & 0x7f]);
        };
        match color {
            LedColor::Off => palette(0),
            LedColor::Palette(v)
            | LedColor::Rgb {
                palette: Some(v), ..
            } => palette(v),
            // A colour of its own, by SysEx: 03 for the lighting command,
            // 03 again for an RGB colour, then the light and its levels from
            // 0 to 127.
            // Levels of 0 to 63, which it takes doubled.
            LedColor::Levels(levels) => {
                let [r, g, b] = levels.map(|level| level.min(63) * 2);
                out.extend_from_slice(&Self::sysex(&[0x03, 0x03, index, r, g, b]));
            }
            LedColor::Rgb {
                rgb: [r, g, b],
                palette: None,
            } => {
                out.extend_from_slice(&Self::sysex(&[0x03, 0x03, index, r >> 1, g >> 1, b >> 1]));
            }
        }
    }

    fn targets(&self) -> Vec<LedTarget> {
        let grid = (1..=8).flat_map(|y| (1..=8).map(move |x| LedTarget::Grid(x, y)));
        grid.chain((1..=8).map(LedTarget::Side))
            .chain((1..=8).map(LedTarget::Top))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programmer_mode() {
        assert_eq!(
            MiniMk3.init_messages()[1],
            [0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x0e, 0x01, 0xf7]
        );
        assert_eq!(
            MiniMk3.exit_messages()[0],
            [0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x0e, 0x00, 0xf7]
        );
    }

    #[test]
    fn presses() {
        let p = MiniMk3;
        assert_eq!(
            p.decode(&[0x90, 11, 100]),
            Some(DeviceInput::Pad {
                x: 1,
                y: 1,
                velocity: 100
            })
        );
        assert_eq!(
            p.decode(&[0x90, 88, 0]),
            Some(DeviceInput::Pad {
                x: 8,
                y: 8,
                velocity: 0
            })
        );
        assert_eq!(
            p.decode(&[0x80, 45, 64]),
            Some(DeviceInput::Pad {
                x: 5,
                y: 4,
                velocity: 0
            })
        );
        assert_eq!(
            p.decode(&[0xb0, 89, 127]),
            Some(DeviceInput::Side {
                n: 8,
                pressed: true
            })
        );
        assert_eq!(
            p.decode(&[0xb0, 91, 0]),
            Some(DeviceInput::Top {
                n: 1,
                pressed: false
            })
        );
        assert_eq!(p.decode(&[0x91, 11, 100]), None);
        assert_eq!(p.decode(&[0xf8]), None);
    }

    #[test]
    fn lights() {
        let p = MiniMk3;
        let mut out = Vec::new();
        p.led_message(LedTarget::Grid(1, 1), LedColor::Palette(5), &mut out);
        assert_eq!(out, [0x90, 11, 5]);
        p.led_message(LedTarget::Side(8), LedColor::Off, &mut out);
        assert_eq!(out, [0xb0, 89, 0]);
        p.led_message(
            LedTarget::Top(2),
            LedColor::Rgb {
                rgb: [1, 2, 3],
                palette: Some(21),
            },
            &mut out,
        );
        assert_eq!(out, [0xb0, 92, 21]);
        p.led_message(
            LedTarget::Grid(8, 8),
            LedColor::Rgb {
                rgb: [255, 128, 0],
                palette: None,
            },
            &mut out,
        );
        assert_eq!(
            out,
            [
                0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x03, 0x03, 88, 127, 64, 0, 0xf7
            ]
        );
        p.led_message(
            LedTarget::Grid(1, 2),
            LedColor::Levels([27, 63, 0]),
            &mut out,
        );
        assert_eq!(
            out,
            [
                0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x03, 0x03, 21, 54, 126, 0, 0xf7
            ]
        );
        p.led_message(LedTarget::Grid(9, 1), LedColor::Palette(5), &mut out);
        assert!(out.is_empty());
    }
}
