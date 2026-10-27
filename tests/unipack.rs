//! A small UniPack read whole, from a folder and from a zip archive, and
//! its sounds decoded.

use std::io::Write;
use std::path::{Path, PathBuf};

use padwire::engine::decode::decode_all;
use padwire::formats::{self, Cover};
use padwire::model::{LedAction, LedColor, LedProgram, LedTarget, LoopMode};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unipack")
}

fn check(cover: &Cover) {
    let project = &cover.project;
    assert_eq!(project.title, "PadWire Test");
    assert_eq!(project.author.as_deref(), Some("PadWire"));
    assert_eq!(project.size, (8, 8));
    assert_eq!(project.pages.len(), 2);
    // kick.wav and Snare.WAV, each once, found whatever the case.
    assert_eq!(project.samples.len(), 2);

    // `1 1 1`: the top left pad, two sounds in turn and two lights.
    let pad = &project.pages[0].pads[&(1, 8)];
    assert_eq!(pad.sounds.len(), 2);
    assert_ne!(pad.sounds[0].sample, pad.sounds[1].sample);
    assert_eq!(pad.leds.len(), 2);
    let LedProgram::Sequence(first) = &pad.leds[0];
    assert_eq!(first.duration_ms, 100);
    assert_eq!(
        first.events[0].action,
        LedAction::Set(
            LedTarget::Grid(1, 8),
            LedColor::Rgb {
                rgb: [255, 0, 0],
                palette: Some(5)
            }
        )
    );
    assert_eq!(
        first.events[2].action,
        LedAction::Set(LedTarget::Side(8), LedColor::Palette(21))
    );

    // `1 8 8`: the bottom right pad, looping until let go.
    let pad = &project.pages[0].pads[&(8, 1)];
    assert_eq!(pad.sounds[0].loop_mode, LoopMode::Infinite);
    let LedProgram::Sequence(lights) = &pad.leds[0];
    assert_eq!(lights.loop_mode, LoopMode::Infinite);

    // `2 4 5 kick.wav 1 1`: a wormhole back to the first page.
    let pad = &project.pages[1].pads[&(5, 5)];
    assert_eq!(pad.sounds[0].wormhole, Some(0));

    // missing.wav.
    assert_eq!(cover.warnings.len(), 1, "{:?}", cover.warnings);
    assert!(cover.warnings[0].contains("missing.wav"));
}

#[test]
fn from_a_folder() {
    let cover = formats::open(&fixture()).unwrap();
    check(&cover);

    let (bank, failures) = decode_all(&*cover.files, &cover.project.samples, 48_000, &|_, _| {});
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(bank.samples.len(), 2);
    // 50 ms each, at 48 kHz in stereo, the 44.1 kHz one resampled.
    for sample in &bank.samples {
        let frames = sample.len() / 2;
        assert!((2395..=2400).contains(&frames), "{frames} frames");
    }
}

#[test]
fn from_a_zip_with_a_folder_inside() {
    let archive = std::env::temp_dir().join(format!("padwire-test-{}.zip", std::process::id()));
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for path in [
            "info",
            "keySound",
            "sounds/kick.wav",
            "sounds/Snare.WAV",
            "keyLED/1 1 1",
            "keyLED/1 1 1 1 b",
            "keyLED/1 8 8 0",
        ] {
            zip.start_file(format!("My Pack/{path}"), options).unwrap();
            zip.write_all(&std::fs::read(fixture().join(path)).unwrap())
                .unwrap();
        }
        zip.finish().unwrap();
    }
    let cover = formats::open(&archive);
    std::fs::remove_file(&archive).unwrap();
    let cover = cover.unwrap();
    check(&cover);
    let (_, failures) = decode_all(&*cover.files, &cover.project.samples, 48_000, &|_, _| {});
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn not_a_cover() {
    let empty = std::env::temp_dir().join(format!("padwire-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let opened = formats::open(&empty);
    std::fs::remove_dir(&empty).unwrap();
    assert!(matches!(opened, Err(formats::Error::NotACover)));
}
