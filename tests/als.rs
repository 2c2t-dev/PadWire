//! A small Ableton cover read whole: two pages set by a macro, samples
//! found from Live 9 and Live 12 references, and its sounds decoded.
//!
//! `PADWIRE_ALS_COVER=path/to/set.als` also reads a real cover, which is
//! not in the repository.

use std::path::{Path, PathBuf};

use padwire::engine::decode::decode_all;
use padwire::formats::{self, Format};
use padwire::model::{LoopMode, PlaybackMode};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/als/Cover")
}

#[test]
fn a_cover_of_two_pages() {
    let set = fixture().join("Open Here/Cover.als");
    let cover = formats::open(&set).unwrap();
    assert_eq!(cover.format, Format::Ableton);
    let project = &cover.project;
    assert_eq!(project.title, "Cover");
    // The macro is mapped to notes 100 and 101: two pages.
    assert_eq!(project.pages.len(), 2);
    let path = |page: usize, pad: (u8, u8)| {
        let sound = &project.pages[page].pads[&pad].sounds[0];
        PathBuf::from(&project.samples[sound.sample].path)
    };

    // Note 36, the bottom left pad: its rack plays a sample a page.
    let first = &project.pages[0].pads[&(1, 1)];
    // Up a folder from the set's, by its Live 9 relative path.
    assert_eq!(path(0, (1, 1)), fixture().join("Page 1/1.wav"));
    assert_eq!(first.playback, PlaybackMode::OneShot);
    assert_eq!(first.choke_group, Some(1));
    assert!((first.sounds[0].gain - 0.501).abs() < 0.001, "-6 dB");
    let second = &project.pages[1].pads[&(1, 1)];
    // By the end of its old Windows path.
    assert_eq!(path(1, (1, 1)), fixture().join("Page 2/1.wav"));
    assert_eq!(second.playback, PlaybackMode::Gate { release_ms: 200 });
    assert_eq!(
        project.samples[second.sounds[0].sample].range,
        Some((100, 999))
    );
    assert_eq!(second.sounds[0].loop_mode, LoopMode::Once);

    // Note 99, the top right pad: no rack, the same on both pages; its name
    // is in both folders, and its size tells which.
    assert_eq!(path(0, (8, 8)), fixture().join("Page 2/1.wav"));
    assert_eq!(path(1, (8, 8)), fixture().join("Page 2/1.wav"));
    assert_eq!(
        project.pages[0].pads[&(8, 8)].playback,
        PlaybackMode::OneShot
    );

    // Note 100 is not on the grid.
    assert!(
        cover.warnings.iter().any(|w| w.contains("not on the grid")),
        "{:?}",
        cover.warnings
    );

    // The whole of Page 2/1.wav, and 900 frames of it at 48 kHz.
    let (bank, failures) = decode_all(&*cover.files, &project.samples, 48_000, &|_, _| {});
    assert!(failures.is_empty(), "{failures:?}");
    let mut lengths: Vec<usize> = bank.samples.iter().map(|s| s.len() / 2).collect();
    lengths.sort();
    assert_eq!(lengths, [900, 2400, 4800]);
}

#[test]
fn the_folder_of_a_cover() {
    // One set in the folder of the folder: it is the one opened.
    let cover = formats::open(&fixture()).unwrap();
    assert_eq!(cover.project.title, "Cover");
}

#[test]
fn a_real_cover() {
    let Some(set) = std::env::var_os("PADWIRE_ALS_COVER") else {
        return;
    };
    let cover = formats::open(Path::new(&set)).unwrap();
    let sounds: usize = cover.project.pages.iter().map(|page| page.pads.len()).sum();
    eprintln!(
        "{}: {} pages, {sounds} pads, {} samples, warnings {:?}",
        cover.project.title,
        cover.project.pages.len(),
        cover.project.samples.len(),
        cover.warnings
    );
    assert!(sounds > 0);
}

#[test]
fn lights_from_midi_effects() {
    use padwire::engine::midifx::{MidiFx, Note};

    let cover = formats::open(&fixture().join("Open Here/Cover.als")).unwrap();
    let lights = cover
        .project
        .midi_lights
        .as_ref()
        .expect("a track of lights");
    assert_eq!(lights.tempo, 120.0);
    assert_eq!(lights.page_notes, 100);
    // A Random device, which notes go through unchanged.
    assert!(
        cover.warnings.iter().any(|w| w.contains("MidiRandom")),
        "{:?}",
        cover.warnings
    );

    let mut fx = MidiFx::new(lights);
    let mut out = Vec::new();
    let mut at = |fx: &mut MidiFx, ms: f64| {
        fx.advance(ms, &mut out);
        std::mem::take(&mut out)
    };
    let on = |note, velocity| Note {
        note,
        velocity,
        layer: 0,
    };
    let off = |note| Note {
        note,
        velocity: 0,
        layer: 0,
    };

    // Page 1: the bottom left pad starts a column going up, a 16th apart
    // at 120 BPM, each note one step long, at velocity 5.
    let mut started = Vec::new();
    fx.input(on(36, 127), 0.0, &mut started);
    assert_eq!(started, [on(36, 5)]);
    assert_eq!(at(&mut fx, 125.0), [off(36), on(40, 5)]);
    assert_eq!(at(&mut fx, 250.0), [off(40), on(44, 5)]);
    assert_eq!(at(&mut fx, 375.0), [off(44)]);
    assert_eq!(fx.next_due(), None);

    // Page 2: every pad a semitone up for 300 ms; the Pitch that is off
    // does nothing.
    fx.set_page(1);
    let mut started = Vec::new();
    fx.input(on(50, 127), 1000.0, &mut started);
    assert_eq!(started, [on(51, 127)]);
    assert_eq!(at(&mut fx, 1300.0), [off(51)]);
}
