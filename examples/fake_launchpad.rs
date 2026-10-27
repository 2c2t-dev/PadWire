//! A Launchpad Mini MK3 that is not there: virtual MIDI ports by the names
//! of the real one's, to play PadWire without the hardware.
//!
//! Run it beside PadWire, which connects to it within a second:
//!
//! ```sh
//! cargo run --example fake_launchpad
//! ```
//!
//! Then type, a line at a time:
//!
//! - `t X Y` to tap the pad at column X, row Y, from 1 at the bottom left;
//! - `p X Y` and `r X Y` to press it and let it go;
//! - `s N` to tap the round button N of the side, 1 at the bottom;
//! - `g` to see the grid as lit; `q` to quit.
//!
//! What PadWire lights is printed as it comes.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use midir::os::unix::{VirtualInput, VirtualOutput};
use midir::{MidiInput, MidiOutput};
use padwire::device::layout::programmer;

type Lit = Arc<Mutex<BTreeMap<u8, String>>>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = "Fake Launchpad Mini MK3";
    let lit: Lit = Arc::default();

    // What PadWire reads presses from…
    let mut presses = MidiOutput::new(client)?.create_virtual("LPMiniMK3 MI")?;
    // …and what it lights.
    let _lights = MidiInput::new(client)?.create_virtual(
        "LPMiniMK3 MI",
        {
            let lit = lit.clone();
            move |_, message, _| received(message, &lit)
        },
        (),
    )?;
    println!("{client} ready: t X Y, p X Y, r X Y, s N, g, q");

    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let words: Vec<u8> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|w| w.parse().ok())
            .collect();
        let mut send = |message: &[u8]| {
            if let Err(e) = presses.send(message) {
                eprintln!("not sent: {e}");
            }
        };
        match (line.split_whitespace().next(), words.as_slice()) {
            (Some("t"), &[x, y]) => {
                send(&[0x90, programmer::note(x, y), 100]);
                std::thread::sleep(Duration::from_millis(100));
                send(&[0x90, programmer::note(x, y), 0]);
            }
            (Some("p"), &[x, y]) => send(&[0x90, programmer::note(x, y), 100]),
            (Some("r"), &[x, y]) => send(&[0x90, programmer::note(x, y), 0]),
            (Some("s"), &[n]) => {
                send(&[0xb0, programmer::side_cc(n), 127]);
                send(&[0xb0, programmer::side_cc(n), 0]);
            }
            (Some("g"), _) => print_grid(&lit),
            (Some("q"), _) => break,
            _ => println!("t X Y, p X Y, r X Y, s N, g, q"),
        }
    }
    Ok(())
}

fn received(message: &[u8], lit: &Lit) {
    let mut lit = lit.lock().unwrap();
    match *message {
        [0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x0e, mode, 0xf7] => {
            println!("{} mode", if mode == 1 { "Programmer" } else { "Live" });
        }
        [
            0xf0,
            0x00,
            0x20,
            0x29,
            0x02,
            0x0d,
            0x03,
            0x03,
            index,
            r,
            g,
            b,
            0xf7,
        ] => {
            println!("light {index}: rgb {r} {g} {b}");
            lit.insert(index, "rgb".into());
        }
        [0x90 | 0xb0, index, velocity] => {
            println!(
                "light {index}: {}",
                if velocity == 0 {
                    "off".to_owned()
                } else {
                    format!("velocity {velocity}")
                }
            );
            if velocity == 0 {
                lit.remove(&index);
            } else {
                lit.insert(index, velocity.to_string());
            }
        }
        _ => println!("{message:02x?}"),
    }
}

fn print_grid(lit: &Lit) {
    let lit = lit.lock().unwrap();
    let cell = |index: u8| {
        lit.get(&index)
            .map_or("  .".to_owned(), |v| format!("{v:>3}"))
    };
    let top: String = (1..=8).map(|n| cell(programmer::top_cc(n))).collect();
    println!("{top}");
    for y in (1..=8).rev() {
        let row: String = (1..=8).map(|x| cell(programmer::note(x, y))).collect();
        println!("{row} |{}", cell(programmer::side_cc(y)));
    }
}
