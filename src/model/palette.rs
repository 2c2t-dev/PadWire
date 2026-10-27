//! The 128 colours a Launchpad lights for a velocity, and how they look on
//! screen.
//!
//! The table holds what the hardware puts out, in the 6-bit levels of its
//! LEDs (0 to 63). It comes from Apollo Studio, by mat1jaczyyy, under the
//! BSD 3-Clause licence: Copyright (c) 2019, mat1jaczyyy. It is the same on
//! the Mini MK3, the X and the Pro MK3; the MK2 is still to be checked.

/// The colour of each velocity, as the LED levels of its red, green and blue.
pub const NOVATION: [[u8; 3]; 128] = [
    [0, 0, 0],
    [7, 7, 7],
    [31, 31, 31],
    [63, 63, 63],
    [63, 18, 18],
    [63, 0, 0],
    [21, 0, 0],
    [6, 0, 0],
    [63, 46, 26],
    [63, 20, 0],
    [21, 7, 0],
    [9, 6, 0],
    [63, 63, 18],
    [63, 63, 0],
    [21, 21, 0],
    [6, 6, 0],
    [33, 63, 18],
    [20, 63, 0],
    [7, 21, 0],
    [4, 10, 0],
    [18, 63, 18],
    [0, 63, 0],
    [0, 21, 0],
    [0, 6, 0],
    [18, 63, 23],
    [0, 63, 6],
    [0, 21, 3],
    [0, 6, 0],
    [18, 63, 33],
    [0, 63, 21],
    [0, 21, 7],
    [0, 7, 4],
    [18, 63, 45],
    [0, 63, 37],
    [0, 21, 13],
    [0, 6, 4],
    [18, 48, 63],
    [0, 41, 63],
    [0, 16, 20],
    [0, 3, 6],
    [18, 33, 63],
    [0, 21, 63],
    [0, 7, 21],
    [0, 1, 6],
    [18, 18, 63],
    [0, 0, 63],
    [0, 0, 21],
    [0, 0, 6],
    [33, 18, 63],
    [20, 0, 63],
    [6, 0, 24],
    [3, 0, 11],
    [63, 18, 63],
    [63, 0, 63],
    [21, 0, 21],
    [6, 0, 6],
    [63, 18, 33],
    [63, 0, 20],
    [21, 0, 7],
    [8, 0, 4],
    [63, 5, 0],
    [37, 13, 0],
    [29, 20, 0],
    [16, 24, 0],
    [0, 14, 0],
    [0, 21, 13],
    [0, 20, 31],
    [0, 0, 63],
    [0, 17, 19],
    [9, 0, 50],
    [31, 31, 31],
    [7, 7, 7],
    [63, 0, 0],
    [46, 63, 11],
    [43, 58, 1],
    [24, 63, 2],
    [3, 34, 0],
    [0, 63, 33],
    [0, 41, 63],
    [0, 10, 63],
    [15, 0, 63],
    [30, 0, 63],
    [43, 6, 30],
    [15, 8, 0],
    [63, 18, 0],
    [33, 55, 1],
    [28, 63, 5],
    [0, 63, 0],
    [14, 63, 9],
    [21, 63, 27],
    [13, 63, 50],
    [22, 34, 63],
    [12, 20, 48],
    [33, 31, 57],
    [52, 7, 63],
    [63, 0, 22],
    [63, 31, 0],
    [45, 43, 0],
    [35, 63, 0],
    [32, 22, 1],
    [14, 10, 0],
    [4, 18, 3],
    [3, 19, 13],
    [5, 5, 10],
    [5, 7, 22],
    [25, 14, 6],
    [41, 0, 2],
    [54, 20, 15],
    [53, 26, 6],
    [63, 55, 9],
    [39, 55, 11],
    [25, 44, 3],
    [7, 7, 11],
    [54, 63, 26],
    [31, 63, 46],
    [38, 37, 63],
    [35, 25, 63],
    [15, 15, 15],
    [28, 28, 28],
    [55, 63, 63],
    [39, 0, 0],
    [13, 0, 0],
    [6, 51, 0],
    [1, 16, 0],
    [45, 43, 0],
    [15, 12, 0],
    [44, 23, 0],
    [18, 5, 0],
];

/// How a velocity looks on screen, in 8-bit sRGB.
///
/// An LED at a quarter of its level still glows plainly, while a quarter of
/// a screen's level is near black, so the levels are lifted by a gamma
/// before they are drawn.
pub fn display_rgb(velocity: u8) -> [u8; 3] {
    NOVATION[usize::from(velocity & 0x7f)].map(level_to_display)
}

/// An LED's level, 0 to 63, as a screen's 0 to 255.
pub fn level_to_display(level: u8) -> u8 {
    if level == 0 {
        return 0;
    }
    let linear = f32::from(level.min(63)) / 63.0;
    (linear.powf(1.0 / 2.2) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmarks() {
        assert_eq!(display_rgb(0), [0, 0, 0]);
        assert_eq!(display_rgb(3), [255, 255, 255]);
        assert_eq!(display_rgb(5), [255, 0, 0]);
        assert_eq!(display_rgb(21), [0, 255, 0]);
        assert_eq!(display_rgb(45), [0, 0, 255]);
    }
}
