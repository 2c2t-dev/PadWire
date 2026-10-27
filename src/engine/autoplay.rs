//! A cover played through by itself, or learnt by it.
//!
//! Playing it, a thread of its own presses the pads and turns the pages at
//! their times, as if someone did. Practising it, the pads to press next
//! light up as guides, the ones after more dimly, and it goes on once they
//! are pressed, at the player's pace: the page buttons are guided the same
//! way when the page is to turn.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::session::{Session, button_of_page};
use crate::model::{AutoAction, AutoPlay, LedColor, LedTarget};

/// How a cover is played through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    /// By itself.
    Play,
    /// By the player, guided.
    Practice,
}

/// What the thread is told while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoInput {
    Stop,
    /// The player pressed a pad.
    Pressed(u8, u8),
    /// The page turned.
    Page(usize),
}

/// Presses this close together are pressed together.
const TOGETHER_MS: f64 = 40.0;
/// The guides: green for what to press now, dim green for after.
const NOW: LedColor = LedColor::Palette(21);
const NEXT: LedColor = LedColor::Palette(23);

/// A step of practice: a page to turn to, or pads to press together.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Page(usize),
    Pads(Vec<(u8, u8)>),
}

fn steps(autoplay: &AutoPlay) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    let mut started = f64::NEG_INFINITY;
    for event in &autoplay.events {
        match event.action {
            AutoAction::Page { page, .. } => steps.push(Step::Page(page)),
            AutoAction::Press(x, y) => match steps.last_mut() {
                Some(Step::Pads(pads)) if event.at_ms - started <= TOGETHER_MS => {
                    if !pads.contains(&(x, y)) {
                        pads.push((x, y));
                    }
                }
                _ => {
                    started = event.at_ms;
                    steps.push(Step::Pads(vec![(x, y)]));
                }
            },
            AutoAction::Release(..) => {}
        }
    }
    steps
}

/// Run a cover through, on the calling thread, until it ends or is told to
/// stop.
pub fn run(
    session: &Session,
    autoplay: &Arc<AutoPlay>,
    mode: AutoMode,
    inputs: &Receiver<AutoInput>,
) {
    match mode {
        AutoMode::Play => play(session, autoplay, inputs),
        AutoMode::Practice => practice(session, autoplay, inputs),
    }
}

fn play(session: &Session, autoplay: &AutoPlay, inputs: &Receiver<AutoInput>) {
    let start = Instant::now();
    for event in &autoplay.events {
        // Waiting is listening: a stop ends it at once.
        let due = start + Duration::from_secs_f64(event.at_ms.max(0.0) / 1000.0);
        loop {
            let wait = due.saturating_duration_since(Instant::now());
            match inputs.recv_timeout(wait) {
                Ok(AutoInput::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        match event.action {
            AutoAction::Press(x, y) => session.press(x, y),
            AutoAction::Release(x, y) => session.release(x, y),
            AutoAction::Page {
                page,
                restart_sounds,
            } => {
                session.set_page(page);
                if restart_sounds {
                    session.restart_sounds();
                }
            }
        }
    }
}

fn practice(session: &Session, autoplay: &AutoPlay, inputs: &Receiver<AutoInput>) {
    let steps = steps(autoplay);
    let on_top = session.first_page_on_top();
    let guide = |step: &Step, color: LedColor, into: &mut Vec<(LedTarget, LedColor)>| match step {
        Step::Pads(pads) => into.extend(pads.iter().map(|&(x, y)| (LedTarget::Grid(x, y), color))),
        Step::Page(page) => {
            if let Some(n) = button_of_page(*page, on_top) {
                into.push((LedTarget::Side(n), color));
            }
        }
    };
    for (i, step) in steps.iter().enumerate() {
        if let Step::Page(page) = step
            && session.page() == Some(*page)
        {
            continue;
        }
        let mut guides = Vec::new();
        if let Some(next) = steps.get(i + 1) {
            guide(next, NEXT, &mut guides);
        }
        // What to do now over what comes after, where both are the same pad.
        guide(step, NOW, &mut guides);
        session.set_guides(guides);
        let mut left: Vec<(u8, u8)> = match step {
            Step::Pads(pads) => pads.clone(),
            Step::Page(_) => Vec::new(),
        };
        loop {
            match inputs.recv() {
                Ok(AutoInput::Stop) | Err(_) => {
                    session.set_guides(Vec::new());
                    return;
                }
                Ok(AutoInput::Pressed(x, y)) => {
                    left.retain(|&pad| pad != (x, y));
                    if matches!(step, Step::Pads(_)) && left.is_empty() {
                        break;
                    }
                }
                Ok(AutoInput::Page(page)) => {
                    if *step == Step::Page(page) {
                        break;
                    }
                }
            }
        }
    }
    session.set_guides(Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AutoEvent;

    #[test]
    fn steps_of_practice() {
        let event = |at_ms, action| AutoEvent { at_ms, action };
        let autoplay = AutoPlay {
            events: vec![
                event(
                    0.0,
                    AutoAction::Page {
                        page: 1,
                        restart_sounds: true,
                    },
                ),
                event(0.0, AutoAction::Press(1, 1)),
                event(30.0, AutoAction::Press(2, 1)),
                event(30.0, AutoAction::Release(1, 1)),
                event(500.0, AutoAction::Press(3, 3)),
                event(510.0, AutoAction::Press(3, 3)),
            ],
        };
        assert_eq!(
            steps(&autoplay),
            [
                Step::Page(1),
                Step::Pads(vec![(1, 1), (2, 1)]),
                Step::Pads(vec![(3, 3)])
            ]
        );
    }
}
