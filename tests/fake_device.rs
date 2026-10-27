//! A cover played from a Launchpad that is not there: virtual MIDI ports
//! by the names of a Mini MK3's, pressed, and the lights they get back.
//!
//! Skipped where there is no ALSA sequencer.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use midir::os::unix::{VirtualInput, VirtualOutput};
use midir::{MidiInput, MidiOutput};
use padwire::device::profiles::DeviceInput;
use padwire::device::{DeviceListener, DeviceManager, DeviceOutput};
use padwire::engine::audio::Bank;
use padwire::engine::led;
use padwire::engine::session::{Event, Session, WindowSink};

struct Listener(Session, mpsc::Sender<Option<&'static str>>);

impl DeviceListener for Listener {
    fn input(&self, input: DeviceInput) {
        self.0.input(input);
    }
    fn connected(&self, model: Option<&'static str>) {
        let _ = self.1.send(model);
    }
}

#[test]
fn pressed_on_a_fake_launchpad() {
    let client = format!("PadWire Test {}", std::process::id());
    let (Ok(fake_out), Ok(fake_in)) = (MidiOutput::new(&client), MidiInput::new(&client)) else {
        eprintln!("no ALSA sequencer: skipped");
        return;
    };
    let mut presses = fake_out.create_virtual("LPMiniMK3 MI").unwrap();
    let (lights_tx, lights) = mpsc::channel::<Vec<u8>>();
    let _lights = fake_in
        .create_virtual(
            "LPMiniMK3 MI",
            move |_, message, _| drop(lights_tx.send(message.to_vec())),
            (),
        )
        .unwrap();

    let cover = padwire::formats::open(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unipack"),
    )
    .unwrap();
    let (events, _events_rx) = async_channel::unbounded::<Event>();
    let (notices, notices_rx) = crossbeam_channel::unbounded();
    let output = DeviceOutput::default();
    let leds = led::spawn(
        vec![
            Box::new(output.clone()),
            Box::new(WindowSink(events.clone())),
        ],
        notices,
    );
    let session = Session::new(None, leds.clone(), notices_rx, events, true);
    session.load(
        Arc::new(cover.project),
        Arc::new(Bank {
            rate: 48_000,
            samples: Vec::new(),
        }),
    );
    let (connected_tx, connected) = mpsc::channel();
    // Only this test's ports, not a real Launchpad that may be plugged in.
    let port = padwire::device::port_names()
        .into_iter()
        .find(|p| p.starts_with(&client));
    let mut manager = DeviceManager::start(
        output,
        leds,
        Arc::new(Listener(session.clone(), connected_tx)),
        port,
    );
    assert_eq!(
        connected.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some("Launchpad Mini MK3")
    );

    let wait_for = |wanted: &[u8]| {
        loop {
            let message = lights
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("no {wanted:02x?}"));
            if message == wanted {
                return;
            }
        }
    };
    // Programmer mode, then the first page's button lit white.
    wait_for(&[0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x0e, 0x01, 0xf7]);
    wait_for(&[0xb0, 89, 3]);

    // The top left pad, note 81: `o 1 1 ff0000 5`, then off and the top
    // side button 21 a tenth of a second later.
    presses.send(&[0x90, 81, 100]).unwrap();
    presses.send(&[0x90, 81, 0]).unwrap();
    wait_for(&[0x90, 81, 5]);
    wait_for(&[0x90, 81, 0]);
    wait_for(&[0xb0, 89, 21]);

    // The second page's button: the light of the first goes back to white
    // only once the program that lit it is over, so the page shows on 79.
    presses.send(&[0xb0, 79, 127]).unwrap();
    wait_for(&[0xb0, 79, 3]);

    manager.shutdown();
    wait_for(&[0xf0, 0x00, 0x20, 0x29, 0x02, 0x0d, 0x0e, 0x00, 0xf7]);
}
