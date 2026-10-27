//! The lights: the programs the pads start, played on a clock in
//! milliseconds on a thread of their own, and what they light sent to the
//! Launchpad and to the window.
//!
//! As in UniPad, a light belongs to the pad whose program lit it last. A
//! program turns off only the lights that are its pad's, and when it is
//! stopped (its pad pressed again, or let go when it loops for ever), every
//! light of its pad goes out. Lights a finished program left on stay on.
//!
//! Under the programs' lights is a base: what shows where none is lit,
//! such as the page buttons.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::midifx::{MidiFx, Note};
use super::session::button_of_page;
use crate::device::layout::drum_rack;
use crate::model::midifx::MidiLights;
use crate::model::{LedAction, LedColor, LedSequence, LedTarget, LoopMode, PadKey};

/// What the lights are told.
#[derive(Debug, Clone)]
pub enum LedCommand {
    /// Start a pad's program, stopping the one it may still be playing.
    Start {
        owner: PadKey,
        sequence: Arc<LedSequence>,
    },
    /// The pad is let go: its programs that loop for ever stop.
    Release { owner: PadKey },
    /// Every program stops, and every light of theirs goes out.
    StopAll,
    /// What shows where no program has lit, in place of what did.
    SetBase(Vec<(LedTarget, LedColor)>),
    /// Say `notice` in so many milliseconds.
    After { ms: u32, notice: Notice },
    /// Send every light again, to a Launchpad just connected.
    Refresh,
    /// Light these guides, under the programs' lights and over the base, in
    /// place of those that were: what to play next, when practising.
    SetGuides(Vec<(LedTarget, LedColor)>),
    /// Play these MIDI effects from now on, or none.
    SetMidiFx {
        lights: Option<Arc<MidiLights>>,
        first_page_on_top: bool,
    },
    /// A note into the MIDI effects: a pad's, or a page button's.
    FxNote { note: u8, velocity: u8 },
    /// The page, which racks mapped to the page buttons follow.
    FxPage(usize),
}

/// What the lights tell back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Turn to a page.
    Page(usize),
}

/// What lights are sent to.
pub trait LedSink: Send {
    /// Lights that changed.
    fn changed(&mut self, changes: &[(LedTarget, LedColor)]);
    /// Every light that is on, everything else being off.
    fn refresh(&mut self, lit: &[(LedTarget, LedColor)]);
}

/// The programs playing, and the lights, without a clock of their own:
/// told the time at every step.
#[derive(Default)]
pub struct Lights {
    running: Vec<Running>,
    lit: HashMap<LedTarget, (PadKey, LedColor)>,
    base: HashMap<LedTarget, LedColor>,
    guides: HashMap<LedTarget, LedColor>,
    /// What the sinks were last told.
    shown: HashMap<LedTarget, LedColor>,
    dirty: Vec<LedTarget>,
    timers: Vec<(u64, Notice)>,
    notices: Vec<Notice>,
    fx: Option<Fx>,
}

/// MIDI effects playing, and where the notes they play light.
struct Fx {
    effects: MidiFx,
    /// The note of the first page's button.
    page_notes: u8,
    first_page_on_top: bool,
    /// The cover's own colours of the velocities, when it has them.
    palette: Option<Arc<Vec<[u8; 3]>>>,
    /// The colour of each layer lit on each light.
    layers: HashMap<LedTarget, BTreeMap<i8, LedColor>>,
    /// What they let out, kept to light.
    out: Vec<Note>,
}

/// Whose the lights of MIDI effects are: no pad's. As on a Launchpad fed by
/// Live, the last note on a light sets it, whatever played it.
const EFFECTS: PadKey = PadKey {
    page: usize::MAX,
    x: 0,
    y: 0,
};

struct Running {
    owner: PadKey,
    sequence: Arc<LedSequence>,
    /// When the time through being played began.
    start: u64,
    /// The next event.
    index: usize,
    /// Times through done.
    pass: u32,
    /// Played through for good, with nothing to wait for, but kept until
    /// let go: a program looping for ever on no delay at all.
    holding: bool,
}

impl Lights {
    pub fn command(&mut self, command: LedCommand, now: u64) {
        match command {
            LedCommand::Start { owner, sequence } => {
                self.stop(|running| running.owner == owner);
                self.running.push(Running {
                    owner,
                    sequence,
                    start: now,
                    index: 0,
                    pass: 0,
                    holding: false,
                });
            }
            LedCommand::Release { owner } => {
                self.stop(|running| {
                    running.owner == owner && running.sequence.loop_mode == LoopMode::Infinite
                });
            }
            LedCommand::StopAll => {
                if let Some(fx) = &mut self.fx {
                    fx.effects.reset();
                    fx.layers.clear();
                }
                self.running.clear();
                self.dirty
                    .extend(self.lit.drain().map(|(target, _)| target));
            }
            LedCommand::SetBase(base) => {
                self.dirty
                    .extend(self.base.drain().map(|(target, _)| target));
                for (target, color) in base {
                    self.base.insert(target, color);
                    self.dirty.push(target);
                }
            }
            LedCommand::After { ms, notice } => self.timers.push((now + u64::from(ms), notice)),
            // The thread answers it, with everything that is lit.
            LedCommand::Refresh => {}
            LedCommand::SetGuides(guides) => {
                self.dirty
                    .extend(self.guides.drain().map(|(target, _)| target));
                for (target, color) in guides {
                    self.guides.insert(target, color);
                    self.dirty.push(target);
                }
            }
            LedCommand::SetMidiFx {
                lights,
                first_page_on_top,
            } => {
                self.fx = lights.map(|lights| Fx {
                    effects: MidiFx::new(&lights),
                    page_notes: lights.page_notes,
                    first_page_on_top,
                    palette: lights.palette.clone(),
                    layers: HashMap::new(),
                    out: Vec::new(),
                });
            }
            LedCommand::FxNote { note, velocity } => {
                if let Some(fx) = &mut self.fx {
                    // Its own time first: what was due goes before.
                    fx.effects.advance(now as f64, &mut fx.out);
                    fx.effects.input(
                        Note {
                            note,
                            velocity,
                            layer: 0,
                        },
                        now as f64,
                        &mut fx.out,
                    );
                }
                self.light_effects();
            }
            LedCommand::FxPage(page) => {
                if let Some(fx) = &mut self.fx {
                    fx.effects.set_page(page);
                }
            }
        }
    }

    /// Light what the MIDI effects let out.
    fn light_effects(&mut self) {
        let Some(fx) = &mut self.fx else { return };
        // Taken out while the lights are set, and put back, emptied, to be
        // filled again without allocating.
        let mut out = std::mem::take(&mut fx.out);
        let (page_notes, on_top) = (fx.page_notes, fx.first_page_on_top);
        let palette = fx.palette.clone();
        for note in out.drain(..) {
            // The Drum Rack layout, all round as Apollo Studio lays it out:
            // the top row from 28, the side buttons from the page notes.
            let target = if let Some((x, y)) = drum_rack::pad(note.note) {
                LedTarget::Grid(x, y)
            } else if (28..=35).contains(&note.note) {
                LedTarget::Top(note.note - 27)
            } else if let Some(n) = note
                .note
                .checked_sub(page_notes)
                .and_then(|page| button_of_page(usize::from(page), on_top))
            {
                LedTarget::Side(n)
            } else {
                continue;
            };
            // The colour of each layer lit on the light; the highest shows.
            let Some(fx) = &mut self.fx else { return };
            let layers = fx.layers.entry(target).or_default();
            if note.velocity == 0 {
                layers.remove(&note.layer);
            } else {
                let color = match palette
                    .as_deref()
                    .and_then(|p| p.get(usize::from(note.velocity)))
                {
                    Some(&levels) => LedColor::Levels(levels),
                    None => LedColor::Palette(note.velocity),
                };
                layers.insert(note.layer, color);
            }
            match layers.last_key_value().map(|(_, &color)| color) {
                Some(color) => self.light(EFFECTS, target, color),
                None => {
                    if self.lit.remove(&target).is_some() {
                        self.dirty.push(target);
                    }
                }
            }
        }
        if let Some(fx) = &mut self.fx {
            fx.out = out;
        }
    }

    /// Stop the programs `which` picks, and put out their pads' lights.
    fn stop(&mut self, which: impl Fn(&Running) -> bool) {
        let mut owners = Vec::new();
        self.running.retain(|running| {
            let stop = which(running);
            if stop {
                owners.push(running.owner);
            }
            !stop
        });
        if owners.is_empty() {
            return;
        }
        let dirty = &mut self.dirty;
        self.lit.retain(|target, (owner, _)| {
            let keep = !owners.contains(owner);
            if !keep {
                dirty.push(*target);
            }
            keep
        });
    }

    /// Play everything due by `now`.
    pub fn advance(&mut self, now: u64) {
        if let Some(fx) = &mut self.fx {
            fx.effects.advance(now as f64, &mut fx.out);
            self.light_effects();
        }
        let mut due = Vec::new();
        self.timers.retain(|&(at, notice)| {
            if at <= now {
                due.push(notice);
            }
            at > now
        });
        self.notices.extend(due);

        let mut finished = Vec::new();
        for i in 0..self.running.len() {
            loop {
                let running = &mut self.running[i];
                if running.holding {
                    break;
                }
                let sequence = Arc::clone(&running.sequence);
                if running.index >= sequence.events.len() {
                    let pass = running.pass + 1;
                    if sequence
                        .loop_mode
                        .passes()
                        .is_some_and(|passes| pass >= passes)
                    {
                        finished.push(i);
                        break;
                    }
                    let duration = u64::from(sequence.duration_ms);
                    if duration == 0 {
                        // Over and over in no time: once is as good, and
                        // kept, so that letting go turns it off.
                        if sequence.loop_mode == LoopMode::Infinite {
                            running.holding = true;
                            break;
                        }
                        running.pass = pass;
                        running.index = 0;
                        continue;
                    }
                    let end = running.start + duration;
                    if end > now {
                        break;
                    }
                    running.pass = pass;
                    running.start = end;
                    // Far behind, as after a stall: the passes missed are
                    // skipped rather than played in a burst.
                    if sequence.loop_mode == LoopMode::Infinite && now - running.start >= duration {
                        running.start += (now - running.start) / duration * duration;
                    }
                    running.index = 0;
                    continue;
                }
                let event = sequence.events[running.index];
                if running.start + u64::from(event.at_ms) > now {
                    break;
                }
                running.index += 1;
                let owner = running.owner;
                match event.action {
                    LedAction::Set(target, color) => self.light(owner, target, color),
                    LedAction::Page(page) => self.notices.push(Notice::Page(page)),
                }
            }
        }
        for i in finished.into_iter().rev() {
            self.running.remove(i);
        }
    }

    fn light(&mut self, owner: PadKey, target: LedTarget, color: LedColor) {
        if color.is_off() {
            if self.lit.get(&target).is_some_and(|(by, _)| *by == owner) {
                self.lit.remove(&target);
                self.dirty.push(target);
            }
        } else {
            self.lit.insert(target, (owner, color));
            self.dirty.push(target);
        }
    }

    /// When something is next due, if anything is.
    pub fn next_due(&self) -> Option<u64> {
        let programs = self
            .running
            .iter()
            .filter(|running| !running.holding)
            .map(|running| match running.sequence.events.get(running.index) {
                Some(event) => running.start + u64::from(event.at_ms),
                None => running.start + u64::from(running.sequence.duration_ms),
            });
        let effects = self
            .fx
            .as_ref()
            .and_then(|fx| fx.effects.next_due())
            .map(|at| at.ceil() as u64);
        programs
            .chain(self.timers.iter().map(|&(at, _)| at))
            .chain(effects)
            .min()
    }

    fn shows(&self, target: LedTarget) -> LedColor {
        match self.lit.get(&target) {
            Some(&(_, color)) => color,
            None => self
                .guides
                .get(&target)
                .or_else(|| self.base.get(&target))
                .copied()
                .unwrap_or_default(),
        }
    }

    /// The lights that changed since last asked.
    pub fn take_changes(&mut self) -> Vec<(LedTarget, LedColor)> {
        let mut dirty = std::mem::take(&mut self.dirty);
        dirty.sort_unstable();
        dirty.dedup();
        let mut changes = Vec::new();
        for target in dirty {
            let color = self.shows(target);
            if self.shown.get(&target).copied().unwrap_or_default() != color {
                if color.is_off() {
                    self.shown.remove(&target);
                } else {
                    self.shown.insert(target, color);
                }
                changes.push((target, color));
            }
        }
        changes
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// Every light that is on.
    pub fn lit(&self) -> Vec<(LedTarget, LedColor)> {
        let mut lit: Vec<_> = self
            .shown
            .iter()
            .map(|(&target, &color)| (target, color))
            .collect();
        lit.sort_unstable_by_key(|&(target, _)| target);
        lit
    }
}

/// What tells the lights' thread what to do.
#[derive(Clone)]
pub struct LedHandle {
    commands: Sender<LedCommand>,
}

impl LedHandle {
    pub fn send(&self, command: LedCommand) {
        // Gone only as the application closes.
        let _ = self.commands.send(command);
    }
}

/// Start the lights' thread, lighting `sinks` and saying its notices to
/// `notices`.
pub fn spawn(sinks: Vec<Box<dyn LedSink>>, notices: Sender<Notice>) -> LedHandle {
    let (commands, receiver) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("padwire-leds".into())
        .spawn(move || run(receiver, sinks, notices))
        .expect("cannot start the lights' thread");
    LedHandle { commands }
}

fn run(commands: Receiver<LedCommand>, mut sinks: Vec<Box<dyn LedSink>>, notices: Sender<Notice>) {
    let epoch = Instant::now();
    let now = || epoch.elapsed().as_millis() as u64;
    let mut lights = Lights::default();
    loop {
        let wait = match lights.next_due() {
            Some(due) => {
                (epoch + Duration::from_millis(due)).saturating_duration_since(Instant::now())
            }
            None => Duration::from_secs(1),
        };
        let mut refresh = false;
        match commands.recv_timeout(wait) {
            Ok(command) => {
                let mut next = Some(command);
                while let Some(command) = next {
                    refresh |= matches!(command, LedCommand::Refresh);
                    lights.command(command, now());
                    next = commands.try_recv().ok();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        lights.advance(now());
        let changes = lights.take_changes();
        if refresh {
            let lit = lights.lit();
            sinks.iter_mut().for_each(|sink| sink.refresh(&lit));
        } else if !changes.is_empty() {
            sinks.iter_mut().for_each(|sink| sink.changed(&changes));
        }
        for notice in lights.take_notices() {
            let _ = notices.send(notice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LedEvent;

    const RED: LedColor = LedColor::Palette(5);
    const GREEN: LedColor = LedColor::Palette(21);

    fn pad(x: u8) -> PadKey {
        PadKey { page: 0, x, y: 1 }
    }

    fn sequence(
        events: &[(u32, LedAction)],
        duration_ms: u32,
        loop_mode: LoopMode,
    ) -> Arc<LedSequence> {
        Arc::new(LedSequence {
            events: events
                .iter()
                .map(|&(at_ms, action)| LedEvent { at_ms, action })
                .collect(),
            duration_ms,
            loop_mode,
        })
    }

    fn set(x: u8, color: LedColor) -> LedAction {
        LedAction::Set(LedTarget::Grid(x, 1), color)
    }

    #[test]
    fn plays_on_time() {
        let mut lights = Lights::default();
        let seq = sequence(
            &[
                (0, set(1, RED)),
                (100, set(1, LedColor::Off)),
                (100, set(2, GREEN)),
            ],
            100,
            LoopMode::Once,
        );
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: seq,
            },
            1000,
        );
        lights.advance(1000);
        assert_eq!(lights.take_changes(), [(LedTarget::Grid(1, 1), RED)]);
        assert_eq!(lights.next_due(), Some(1100));
        lights.advance(1099);
        assert!(lights.take_changes().is_empty());
        lights.advance(1100);
        assert_eq!(
            lights.take_changes(),
            [
                (LedTarget::Grid(1, 1), LedColor::Off),
                (LedTarget::Grid(2, 1), GREEN)
            ]
        );
        // Finished, its light left on.
        lights.advance(5000);
        assert_eq!(lights.next_due(), None);
        assert_eq!(lights.lit(), [(LedTarget::Grid(2, 1), GREEN)]);
    }

    #[test]
    fn loops_and_stops_when_let_go() {
        let mut lights = Lights::default();
        let seq = sequence(
            &[(0, set(1, RED)), (50, set(1, LedColor::Off))],
            100,
            LoopMode::Infinite,
        );
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: seq,
            },
            0,
        );
        lights.advance(0);
        lights.advance(50);
        lights.advance(100);
        lights.take_changes();
        assert_eq!(lights.lit(), [(LedTarget::Grid(1, 1), RED)]);
        lights.command(LedCommand::Release { owner: pad(1) }, 120);
        assert_eq!(
            lights.take_changes(),
            [(LedTarget::Grid(1, 1), LedColor::Off)]
        );
        assert_eq!(lights.next_due(), None);
    }

    #[test]
    fn plays_as_many_times_as_its_loop() {
        let mut lights = Lights::default();
        let seq = sequence(&[(0, LedAction::Page(1))], 10, LoopMode::Times(3));
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: seq,
            },
            0,
        );
        for now in 0..100 {
            lights.advance(now);
        }
        assert_eq!(lights.take_notices().len(), 3);
    }

    #[test]
    fn lights_belong_to_the_pad_that_lit_them() {
        let mut lights = Lights::default();
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: sequence(&[(0, set(3, RED))], 0, LoopMode::Once),
            },
            0,
        );
        lights.command(
            LedCommand::Start {
                owner: pad(2),
                sequence: sequence(&[(0, set(3, LedColor::Off))], 0, LoopMode::Once),
            },
            0,
        );
        lights.advance(0);
        assert_eq!(lights.take_changes(), [(LedTarget::Grid(3, 1), RED)]);
    }

    #[test]
    fn pressed_again_starts_over() {
        let mut lights = Lights::default();
        let seq = sequence(
            &[(0, set(1, RED)), (100, set(2, GREEN))],
            100,
            LoopMode::Once,
        );
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: seq.clone(),
            },
            0,
        );
        lights.advance(0);
        lights.take_changes();
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: seq,
            },
            50,
        );
        lights.advance(50);
        // Put out, then lit again by the new run: no change to send.
        assert!(lights.take_changes().is_empty());
        lights.advance(120);
        assert!(lights.take_changes().is_empty());
        lights.advance(150);
        assert_eq!(lights.take_changes(), [(LedTarget::Grid(2, 1), GREEN)]);
    }

    #[test]
    fn base_shows_under_programs() {
        let mut lights = Lights::default();
        lights.command(LedCommand::SetBase(vec![(LedTarget::Side(8), GREEN)]), 0);
        assert_eq!(lights.take_changes(), [(LedTarget::Side(8), GREEN)]);
        let seq = sequence(
            &[(0, LedAction::Set(LedTarget::Side(8), RED))],
            0,
            LoopMode::Infinite,
        );
        lights.command(
            LedCommand::Start {
                owner: pad(1),
                sequence: seq,
            },
            0,
        );
        lights.advance(0);
        assert_eq!(lights.take_changes(), [(LedTarget::Side(8), RED)]);
        lights.command(LedCommand::Release { owner: pad(1) }, 10);
        assert_eq!(lights.take_changes(), [(LedTarget::Side(8), GREEN)]);
    }

    #[test]
    fn higher_layers_show_over_lower_ones() {
        use crate::model::midifx::{Device, MidiLights, VelocityMap, VelocityMode};
        let fixed = |velocity| {
            Device::Velocity(VelocityMap {
                lowest: 1,
                range: 127,
                out_low: velocity,
                out_high: velocity,
                mode: VelocityMode::Fixed,
            })
        };
        // The same pad lit twice: green on layer 1 for 100 ms, red on
        // layer 0 while held.
        let lights = MidiLights {
            chains: vec![
                vec![
                    fixed(21),
                    Device::NoteLength {
                        ms: 100.0,
                        on_release: false,
                    },
                    Device::Layer(1),
                ],
                vec![fixed(5), Device::Layer(0)],
            ],
            tempo: 120.0,
            page_notes: 100,
            palette: None,
        };
        let mut lights_state = Lights::default();
        lights_state.command(
            LedCommand::SetMidiFx {
                lights: Some(Arc::new(lights)),
                first_page_on_top: true,
            },
            0,
        );
        lights_state.command(
            LedCommand::FxNote {
                note: 36,
                velocity: 127,
            },
            0,
        );
        assert_eq!(
            lights_state.take_changes(),
            [(LedTarget::Grid(1, 1), GREEN)]
        );
        // Layer 1 ends: layer 0 shows through.
        lights_state.advance(100);
        assert_eq!(lights_state.take_changes(), [(LedTarget::Grid(1, 1), RED)]);
        lights_state.command(
            LedCommand::FxNote {
                note: 36,
                velocity: 0,
            },
            200,
        );
        assert_eq!(
            lights_state.take_changes(),
            [(LedTarget::Grid(1, 1), LedColor::Off)]
        );
    }

    #[test]
    fn timers() {
        let mut lights = Lights::default();
        lights.command(
            LedCommand::After {
                ms: 100,
                notice: Notice::Page(2),
            },
            0,
        );
        assert_eq!(lights.next_due(), Some(100));
        lights.advance(99);
        assert!(lights.take_notices().is_empty());
        lights.advance(100);
        assert_eq!(lights.take_notices(), [Notice::Page(2)]);
    }
}
