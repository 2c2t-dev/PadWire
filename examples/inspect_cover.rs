//! What PadWire reads of a cover, without playing it: its pages, its pads
//! and its samples, and what could not be read.
//!
//! ```sh
//! cargo run --release --example inspect_cover -- path/to/cover
//! ```
//!
//! With `--decode`, every sample is decoded too, as before playing; with
//! `--pads`, every pad's files are listed; with `--lights PAGE X Y`, the pad
//! is pressed for 200 ms and what its MIDI effects light is told in time;
//! with `--library DIR`, a folder of Ableton's sounds is looked through as
//! the preferences add one.

use std::path::PathBuf;
use std::time::Instant;

use padwire::engine::decode::decode_all;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let decode = args.iter().any(|arg| arg == "--decode");
    let list = args.iter().any(|arg| arg == "--pads");
    let lights: Option<Vec<usize>> = args.iter().position(|arg| arg == "--lights").map(|at| {
        let numbers = args[at + 1..at + 4]
            .iter()
            .map(|n| n.parse().expect("--lights PAGE X Y"))
            .collect();
        args.drain(at..at + 4);
        numbers
    });
    let all_lights = args.iter().any(|arg| arg == "--lights-all");
    // A folder of Ableton's sounds, as the preferences add.
    let library = args.iter().position(|arg| arg == "--library").map(|at| {
        let dir = PathBuf::from(&args[at + 1]);
        args.drain(at..at + 2);
        dir
    });
    args.retain(|arg| arg != "--decode" && arg != "--pads" && arg != "--lights-all");
    let Some(path) = args.first().map(PathBuf::from) else {
        eprintln!("usage: inspect_cover [--decode] [--pads] [--lights PAGE X Y] COVER");
        std::process::exit(2);
    };

    let started = Instant::now();
    let options = padwire::formats::Options {
        ableton_libraries: library.into_iter().collect(),
    };
    for found in padwire::formats::als::ableton_libraries() {
        println!("Ableton library found: {}", found.display());
    }
    let cover = match padwire::formats::open_with(&path, &options) {
        Ok(cover) => cover,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let project = &cover.project;
    println!(
        "{} ({}), read in {:.2?}",
        project.title,
        cover.format.label(),
        started.elapsed()
    );
    println!(
        "{} pages, {} samples",
        project.pages.len(),
        project.samples.len()
    );
    if let Some(lights) = &project.midi_lights {
        let palette = match &lights.palette {
            Some(palette) => format!(
                "the cover's own (1 = {:?}, 5 = {:?}, 13 = {:?})",
                palette[1], palette[5], palette[13]
            ),
            None => "Novation's".to_owned(),
        };
        println!(
            "lights: {} chains at {} BPM, palette {palette}",
            lights.chains.len(),
            lights.tempo
        );
    }
    if let Some(autoplay) = &project.autoplay {
        use padwire::model::AutoAction;
        let presses = autoplay
            .events
            .iter()
            .filter(|e| matches!(e.action, AutoAction::Press(..)))
            .count();
        let pages: Vec<usize> = autoplay
            .events
            .iter()
            .filter_map(|e| match e.action {
                AutoAction::Page { page, .. } => Some(page + 1),
                _ => None,
            })
            .collect();
        let end = autoplay.events.last().map_or(0.0, |e| e.at_ms) / 1000.0;
        println!("autoplay: {presses} presses over {end:.0} s, pages {pages:?}");
    }
    for (i, page) in project.pages.iter().enumerate() {
        let sounds: usize = page.pads.values().map(|pad| pad.sounds.len()).sum();
        let leds: usize = page.pads.values().map(|pad| pad.leds.len()).sum();
        println!(
            "  page {}: {} pads, {sounds} sounds, {leds} light programs",
            i + 1,
            page.pads.len()
        );
        // The grid, top row first: how many sounds each pad has.
        for y in (1..=8).rev() {
            let row: String = (1..=8)
                .map(
                    |x| match page.pads.get(&(x, y)).map(|pad| pad.sounds.len()) {
                        Some(n) if n > 0 => format!("{n:>2}"),
                        _ => " .".into(),
                    },
                )
                .collect();
            println!("    {row}");
        }
    }
    if list {
        // Every pad's sounds: page, x, y, then the files.
        for (i, page) in project.pages.iter().enumerate() {
            let mut pads: Vec<_> = page.pads.iter().collect();
            pads.sort_by_key(|((x, y), _)| (*y, *x));
            for ((x, y), pad) in pads {
                let files: Vec<&str> = pad
                    .sounds
                    .iter()
                    .map(|s| project.samples[s.sample].path.as_str())
                    .collect();
                println!("pad {} {x} {y} {}", i + 1, files.join(" | "));
            }
        }
    }
    if let Some(pad) = lights {
        press(&cover.project, pad[0] - 1, pad[1] as u8, pad[2] as u8, true);
    }
    if all_lights {
        for page in 0..project.pages.len() {
            for y in (1..=8).rev() {
                for x in 1..=8 {
                    press(&cover.project, page, x, y, false);
                }
            }
        }
    }
    for warning in &cover.warnings {
        println!("warning: {warning}");
    }
    if decode {
        let started = Instant::now();
        let (bank, failures) = decode_all(&*cover.files, &project.samples, 48_000, &|_, _| {});
        let seconds: f64 = bank
            .samples
            .iter()
            .map(|s| s.len() as f64 / 2.0 / 48_000.0)
            .sum();
        let megabytes = bank.samples.iter().map(|s| s.len() * 4).sum::<usize>() as f64 / 1e6;
        println!(
            "decoded in {:.2?}: {seconds:.0} s of sound, {megabytes:.0} MB",
            started.elapsed()
        );
        for failure in failures {
            println!("not decoded: {failure}");
        }
    }
}

/// A pad pressed for 200 ms: what its MIDI effects play, until they stop.
fn press(project: &padwire::model::Project, page: usize, x: u8, y: u8, every: bool) {
    use padwire::device::layout::drum_rack;
    use padwire::engine::midifx::{MidiFx, Note};

    let Some(lights) = &project.midi_lights else {
        println!("this cover has no MIDI effects to light");
        return;
    };
    let mut fx = MidiFx::new(lights);
    fx.set_page(page);
    let note = drum_rack::note(x, y);
    let mut out = Vec::new();
    fx.input(
        Note {
            note,
            velocity: 127,
            layer: 0,
        },
        0.0,
        &mut out,
    );
    let mut timeline: Vec<(f64, Note)> = out.drain(..).map(|n| (0.0, n)).collect();
    let mut released = false;
    while let Some(at) = fx.next_due().filter(|&at| at < 60_000.0) {
        if !released && at >= 200.0 {
            fx.input(
                Note {
                    note,
                    velocity: 0,
                    layer: 0,
                },
                200.0,
                &mut out,
            );
            timeline.extend(out.drain(..).map(|n| (200.0, n)));
            released = true;
            continue;
        }
        fx.advance(at, &mut out);
        timeline.extend(out.drain(..).map(|n| (at, n)));
    }
    if !released {
        fx.input(
            Note {
                note,
                velocity: 0,
                layer: 0,
            },
            200.0,
            &mut out,
        );
        timeline.extend(out.drain(..).map(|n| (200.0, n)));
    }
    if !every {
        // One line: the notes started, in order, and when the last ends.
        let starts: Vec<String> = timeline
            .iter()
            .filter(|(_, n)| n.velocity > 0)
            .map(|(_, n)| format!("{}:{}", n.note, n.velocity))
            .collect();
        let end = timeline.last().map_or(0.0, |(at, _)| *at);
        println!(
            "{} {x} {y} n{note} end={end:.0} {}",
            page + 1,
            starts.join(" ")
        );
        return;
    }
    println!(
        "page {} pad ({x}, {y}), note {note}: {} notes out",
        page + 1,
        timeline.len()
    );
    for (at, note) in timeline {
        let place = match drum_rack::pad(note.note) {
            Some((x, y)) => format!("pad ({x}, {y})"),
            None => format!("note {}", note.note),
        };
        match note.velocity {
            0 => println!("  {at:>8.1} ms  {place} off"),
            v => println!("  {at:>8.1} ms  {place} on, velocity {v}"),
        }
    }
}
