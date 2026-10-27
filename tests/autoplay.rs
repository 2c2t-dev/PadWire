//! A cover played through by itself, and practised, from the small UniPack
//! of the fixtures, the lights watched as the window sees them.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use padwire::engine::audio::Bank;
use padwire::engine::autoplay::AutoMode;
use padwire::engine::led;
use padwire::engine::session::{Event, Session, WindowSink};
use padwire::model::{AutoAction, LedColor, LedTarget};

fn session() -> (Session, async_channel::Receiver<Event>) {
    let cover = padwire::formats::open(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unipack"),
    )
    .unwrap();
    let (events, events_rx) = async_channel::unbounded();
    let (notices, notices_rx) = crossbeam_channel::unbounded();
    let leds = led::spawn(vec![Box::new(WindowSink(events.clone()))], notices);
    let session = Session::new(None, leds, notices_rx, events, true);
    session.load(
        Arc::new(cover.project),
        Arc::new(Bank {
            rate: 48_000,
            samples: Vec::new(),
        }),
    );
    (session, events_rx)
}

/// Wait for an event that `wanted` takes, the others dropped.
fn wait_for(events: &async_channel::Receiver<Event>, wanted: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(event) if wanted(&event) => return event,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("the event waited for never came");
}

fn lit(target: LedTarget, color: LedColor) -> impl Fn(&Event) -> bool {
    move |event| matches!(event, Event::Leds(changes) if changes.contains(&(target, color)))
}

#[test]
fn the_autoplay_file_is_read() {
    let cover = padwire::formats::open(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unipack"),
    )
    .unwrap();
    let autoplay = cover.project.autoplay.expect("an autoPlay");
    let presses = autoplay
        .events
        .iter()
        .filter(|e| matches!(e.action, AutoAction::Press(..)))
        .count();
    assert_eq!(presses, 3);
    assert_eq!(autoplay.events.last().unwrap().at_ms, 100.0);
}

#[test]
fn played_through_to_its_end() {
    let (session, events) = session();
    assert!(session.has_autoplay());
    session.start_autoplay(AutoMode::Play);
    // `c 2`: the second page.
    wait_for(&events, |e| matches!(e, Event::Page { index: 1, .. }));
    wait_for(&events, |e| matches!(e, Event::AutoPlay(None)));
}

#[test]
fn practised_at_the_players_pace() {
    let green = LedColor::Palette(21);
    let (session, events) = session();
    session.start_autoplay(AutoMode::Practice);
    // `o 1 1`: the top left pad, guided; the one after, more dimly.
    wait_for(&events, lit(LedTarget::Grid(1, 8), green));
    session.press(1, 8);
    session.release(1, 8);
    // `t 8 8`.
    wait_for(&events, lit(LedTarget::Grid(8, 1), green));
    session.press(8, 1);
    // `c 2`: the second page's button, the second from the top.
    wait_for(&events, lit(LedTarget::Side(7), green));
    session.set_page(1);
    wait_for(&events, lit(LedTarget::Grid(5, 5), green));
    session.press(5, 5);
    wait_for(&events, |e| matches!(e, Event::AutoPlay(None)));
}
