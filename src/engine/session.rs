//! A cover being played: what a press does, from the Launchpad or the
//! window alike.
//!
//! As in UniPad: pressing a pad stops the sound it was still playing,
//! plays the next of its sounds and starts the next of its lights; letting
//! it go stops a sound that loops for ever, and lights that do. A sound
//! with a wormhole turns the page a moment after it is pressed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::audio::{AudioControl, Bank, Command};
use super::autoplay::{self, AutoInput, AutoMode};
use super::led::{LedCommand, LedHandle, LedSink, Notice};
use crate::device::layout::drum_rack;
use crate::device::profiles::DeviceInput;
use crate::model::{LedColor, LedProgram, LedTarget, LoopMode, PadKey, PlaybackMode, Project};

/// How hard a pad is taken as hit: the Mini MK3's pads always say 127.
const PAD_VELOCITY: u8 = 127;
/// How long after the press a wormhole turns the page, as UniPad waits.
const WORMHOLE_MS: u32 = 100;
/// What the button of the page showing is lit with: white.
const PAGE_COLOR: LedColor = LedColor::Palette(3);

/// What the window is told.
#[derive(Debug, Clone)]
pub enum Event {
    /// Lights that changed.
    Leds(Vec<(LedTarget, LedColor)>),
    /// Every light that is on, everything else being off.
    LedsRefresh(Vec<(LedTarget, LedColor)>),
    Page {
        index: usize,
        count: usize,
    },
    /// The Launchpad connected, by its model's name, or none.
    Device(Option<String>),
    /// The cover playing through by itself or being practised, or neither
    /// any more.
    AutoPlay(Option<AutoMode>),
}

/// The lights, as the window is to show them.
pub struct WindowSink(pub async_channel::Sender<Event>);

impl LedSink for WindowSink {
    fn changed(&mut self, changes: &[(LedTarget, LedColor)]) {
        let _ = self.0.try_send(Event::Leds(changes.to_vec()));
    }

    fn refresh(&mut self, lit: &[(LedTarget, LedColor)]) {
        let _ = self.0.try_send(Event::LedsRefresh(lit.to_vec()));
    }
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    audio: Option<AudioControl>,
    leds: LedHandle,
    events: async_channel::Sender<Event>,
    playing: Option<Playing>,
    first_page_on_top: bool,
    /// The cover played through, while it is.
    auto: Option<AutoRun>,
    /// Counts the runs, so that one ending late does not end the next.
    auto_runs: u64,
}

struct AutoRun {
    mode: AutoMode,
    run: u64,
    inputs: crossbeam_channel::Sender<AutoInput>,
}

struct Playing {
    project: Arc<Project>,
    page: usize,
    /// Where each pad is in its sounds, and in its lights.
    sound_turn: HashMap<PadKey, usize>,
    led_turn: HashMap<PadKey, usize>,
    /// The pads held, by where they are, with the pad of the page they were
    /// pressed on: letting go stops what the press started, whatever page
    /// shows by then.
    held: HashMap<(u8, u8), Held>,
}

struct Held {
    key: PadKey,
    /// How long what it plays takes to fade once let go, if it stops then.
    release: Option<u32>,
}

impl Session {
    /// A session, which turns the page when the lights say so.
    pub fn new(
        audio: Option<AudioControl>,
        leds: LedHandle,
        notices: crossbeam_channel::Receiver<Notice>,
        events: async_channel::Sender<Event>,
        first_page_on_top: bool,
    ) -> Self {
        let session = Session {
            inner: Arc::new(Mutex::new(Inner {
                audio,
                leds,
                events,
                playing: None,
                first_page_on_top,
                auto: None,
                auto_runs: 0,
            })),
        };
        let weak = Arc::downgrade(&session.inner);
        std::thread::Builder::new()
            .name("padwire-notices".into())
            .spawn(move || {
                for notice in notices {
                    let Some(inner) = weak.upgrade() else { return };
                    let session = Session { inner };
                    match notice {
                        Notice::Page(page) => session.set_page(page),
                    }
                }
            })
            .expect("cannot start the notices' thread");
        session
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The rate sounds are to be decoded at, if there is any audio.
    pub fn audio_rate(&self) -> Option<u32> {
        self.lock().audio.as_ref().map(AudioControl::rate)
    }

    /// Play a cover from now on, on its first page.
    pub fn load(&self, project: Arc<Project>, bank: Arc<Bank>) {
        let mut inner = self.lock();
        inner.stop_auto();
        if let Some(audio) = &mut inner.audio {
            audio.send(Command::SetBank(bank));
        }
        inner.leds.send(LedCommand::StopAll);
        inner.leds.send(LedCommand::SetMidiFx {
            lights: project.midi_lights.clone(),
            first_page_on_top: inner.first_page_on_top,
        });
        inner.playing = Some(Playing {
            project,
            page: 0,
            sound_turn: HashMap::new(),
            led_turn: HashMap::new(),
            held: HashMap::new(),
        });
        inner.show_page();
    }

    /// Stop playing the cover.
    pub fn unload(&self) {
        let mut inner = self.lock();
        inner.stop_auto();
        if let Some(audio) = &mut inner.audio {
            audio.send(Command::StopAll);
        }
        inner.leds.send(LedCommand::StopAll);
        inner.leds.send(LedCommand::SetBase(Vec::new()));
        inner.leds.send(LedCommand::SetMidiFx {
            lights: None,
            first_page_on_top: inner.first_page_on_top,
        });
        inner.playing = None;
    }

    pub fn input(&self, input: DeviceInput) {
        match input {
            DeviceInput::Pad { x, y, velocity: 0 } => self.release(x, y),
            DeviceInput::Pad { x, y, .. } => self.press(x, y),
            DeviceInput::Side { n, pressed } => {
                if pressed {
                    self.page_button(n);
                }
                self.page_note(n, pressed);
            }
            DeviceInput::Top { n, pressed: true } => self.top_button(n),
            DeviceInput::Top { .. } => {}
        }
    }

    pub fn press(&self, x: u8, y: u8) {
        let mut inner = self.lock();
        let Inner {
            audio,
            leds,
            playing,
            auto,
            ..
        } = &mut *inner;
        let Some(playing) = playing else { return };
        if let Some(auto) = auto
            && auto.mode == AutoMode::Practice
        {
            let _ = auto.inputs.send(AutoInput::Pressed(x, y));
        }
        if playing.project.midi_lights.is_some() && (1..=8).contains(&x) && (1..=8).contains(&y) {
            leds.send(LedCommand::FxNote {
                note: drum_rack::note(x, y),
                velocity: PAD_VELOCITY,
            });
        }
        let key = PadKey {
            page: playing.page,
            x,
            y,
        };
        let tag = tag(key);
        if let Some(audio) = audio.as_mut() {
            audio.send(Command::StopTag(tag));
        }
        let Some(pad) = playing
            .project
            .pages
            .get(key.page)
            .and_then(|page| page.pads.get(&(x, y)))
        else {
            return;
        };
        let mut release = match pad.playback {
            PlaybackMode::Gate { release_ms } => Some(release_ms),
            PlaybackMode::OneShot => None,
        };
        if !pad.sounds.is_empty() {
            let turn = playing.sound_turn.entry(key).or_default();
            let sound = &pad.sounds[*turn % pad.sounds.len()];
            *turn = (*turn + 1) % pad.sounds.len();
            if sound.loop_mode == LoopMode::Infinite {
                release.get_or_insert(0);
            }
            if let Some(audio) = audio.as_mut() {
                audio.send(Command::Play {
                    sample: sound.sample as u32,
                    gain: sound.gain,
                    passes: sound.loop_mode.passes().unwrap_or(0),
                    tag,
                    choke: pad.choke_group.unwrap_or(0),
                });
            }
            if let Some(page) = sound.wormhole {
                leds.send(LedCommand::After {
                    ms: WORMHOLE_MS,
                    notice: Notice::Page(page),
                });
            }
        }
        if !pad.leds.is_empty() {
            let turn = playing.led_turn.entry(key).or_default();
            let LedProgram::Sequence(sequence) = &pad.leds[*turn % pad.leds.len()];
            *turn = (*turn + 1) % pad.leds.len();
            leds.send(LedCommand::Start {
                owner: key,
                sequence: Arc::clone(sequence),
            });
        }
        playing.held.insert((x, y), Held { key, release });
    }

    pub fn release(&self, x: u8, y: u8) {
        let mut inner = self.lock();
        let Inner {
            audio,
            leds,
            playing,
            ..
        } = &mut *inner;
        let Some(playing) = playing else { return };
        if playing.project.midi_lights.is_some() && (1..=8).contains(&x) && (1..=8).contains(&y) {
            leds.send(LedCommand::FxNote {
                note: drum_rack::note(x, y),
                velocity: 0,
            });
        }
        let Some(held) = playing.held.remove(&(x, y)) else {
            return;
        };
        if let (Some(release_ms), Some(audio)) = (held.release, audio.as_mut()) {
            let frames = (u64::from(release_ms) * u64::from(audio.rate()) / 1000) as u32;
            audio.send(Command::Release {
                tag: tag(held.key),
                frames,
            });
        }
        leds.send(LedCommand::Release { owner: held.key });
    }

    /// The page a round button of the right side turns to, 1 at the bottom.
    pub fn page_of_button(&self, n: u8) -> usize {
        page_of_button(n, self.lock().first_page_on_top)
    }

    fn page_button(&self, n: u8) {
        self.set_page(self.page_of_button(n));
    }

    /// A page button is a note too, which MIDI effects may light from.
    fn page_note(&self, n: u8, pressed: bool) {
        let inner = self.lock();
        let Some(lights) = inner
            .playing
            .as_ref()
            .and_then(|playing| playing.project.midi_lights.as_ref())
        else {
            return;
        };
        let page = page_of_button(n, inner.first_page_on_top);
        let Some(note) = u8::try_from(page)
            .ok()
            .and_then(|page| lights.page_notes.checked_add(page))
        else {
            return;
        };
        inner.leds.send(LedCommand::FxNote {
            note,
            velocity: if pressed { PAD_VELOCITY } else { 0 },
        });
    }

    /// The arrows of the top row turn the pages one by one, for a cover of
    /// more pages than there are buttons on the side.
    fn top_button(&self, n: u8) {
        let page = {
            let inner = self.lock();
            let Some(playing) = &inner.playing else {
                return;
            };
            match n {
                1 | 3 => playing.page.checked_sub(1),
                2 | 4 => Some(playing.page + 1),
                _ => None,
            }
        };
        if let Some(page) = page {
            self.set_page(page);
        }
    }

    pub fn set_page(&self, page: usize) {
        let mut inner = self.lock();
        let Some(playing) = &mut inner.playing else {
            return;
        };
        if page >= playing.project.pages.len() || page == playing.page {
            return;
        }
        playing.page = page;
        inner.show_page();
        if let Some(auto) = &inner.auto
            && auto.mode == AutoMode::Practice
        {
            let _ = auto.inputs.send(AutoInput::Page(page));
        }
    }

    /// The page showing, if a cover is.
    pub fn page(&self) -> Option<usize> {
        self.lock().playing.as_ref().map(|playing| playing.page)
    }

    pub fn first_page_on_top(&self) -> bool {
        self.lock().first_page_on_top
    }

    /// Every pad's sounds and lights from their first again.
    pub fn restart_sounds(&self) {
        if let Some(playing) = &mut self.lock().playing {
            playing.sound_turn.clear();
            playing.led_turn.clear();
        }
    }

    /// Light these guides, in place of those that were.
    pub fn set_guides(&self, guides: Vec<(LedTarget, LedColor)>) {
        self.lock().leds.send(LedCommand::SetGuides(guides));
    }

    /// Whether the cover playing can be played through.
    pub fn has_autoplay(&self) -> bool {
        let inner = self.lock();
        inner
            .playing
            .as_ref()
            .is_some_and(|playing| playing.project.autoplay.is_some())
    }

    /// Play the cover through, or practise it, from its first page; what
    /// ran before stops.
    pub fn start_autoplay(&self, mode: AutoMode) {
        let (autoplay, run, inputs) = {
            let mut inner = self.lock();
            inner.stop_auto();
            let Some(autoplay) = inner
                .playing
                .as_ref()
                .and_then(|p| p.project.autoplay.clone())
            else {
                return;
            };
            inner.auto_runs += 1;
            let run = inner.auto_runs;
            let (sender, inputs) = crossbeam_channel::unbounded();
            inner.auto = Some(AutoRun {
                mode,
                run,
                inputs: sender,
            });
            if let Some(audio) = &mut inner.audio {
                audio.send(Command::StopAll);
            }
            inner.leds.send(LedCommand::StopAll);
            let _ = inner.events.try_send(Event::AutoPlay(Some(mode)));
            (autoplay, run, inputs)
        };
        self.restart_sounds();
        self.set_page(0);
        let session = self.clone();
        let spawned = std::thread::Builder::new()
            .name("padwire-autoplay".into())
            .spawn(move || {
                autoplay::run(&session, &autoplay, mode, &inputs);
                let mut inner = session.lock();
                if inner.auto.as_ref().is_some_and(|auto| auto.run == run) {
                    inner.auto = None;
                    let _ = inner.events.try_send(Event::AutoPlay(None));
                }
            });
        if let Err(e) = spawned {
            log::error!("cannot play the cover through: {e}");
            self.stop_autoplay();
        }
    }

    pub fn stop_autoplay(&self) {
        self.lock().stop_auto();
    }

    pub fn set_first_page_on_top(&self, on_top: bool) {
        let mut inner = self.lock();
        inner.first_page_on_top = on_top;
        if let Some(playing) = &inner.playing {
            inner.leds.send(LedCommand::SetMidiFx {
                lights: playing.project.midi_lights.clone(),
                first_page_on_top: on_top,
            });
        }
        inner.show_page();
    }
}

impl Inner {
    /// Stop the cover playing through, if it is, and tell the window.
    fn stop_auto(&mut self) {
        if let Some(auto) = self.auto.take() {
            let _ = auto.inputs.send(AutoInput::Stop);
            let _ = self.events.try_send(Event::AutoPlay(None));
        }
    }

    /// Light the button of the page showing, and tell the window.
    fn show_page(&mut self) {
        let Some(playing) = &self.playing else { return };
        let count = playing.project.pages.len();
        let base = button_of_page(playing.page, self.first_page_on_top)
            .map(|n| vec![(LedTarget::Side(n), PAGE_COLOR)])
            .unwrap_or_default();
        self.leds.send(LedCommand::SetBase(base));
        if playing.project.midi_lights.is_some() {
            self.leds.send(LedCommand::FxPage(playing.page));
        }
        let _ = self.events.try_send(Event::Page {
            index: playing.page,
            count,
        });
    }
}

/// What a pad's sounds are stopped by.
fn tag(key: PadKey) -> u32 {
    (key.page as u32) << 16 | u32::from(key.x) << 8 | u32::from(key.y)
}

pub fn page_of_button(n: u8, first_page_on_top: bool) -> usize {
    if first_page_on_top {
        usize::from(8 - n.min(8))
    } else {
        usize::from(n.max(1) - 1)
    }
}

pub fn button_of_page(page: usize, first_page_on_top: bool) -> Option<u8> {
    let page = u8::try_from(page).ok().filter(|&page| page < 8)?;
    Some(if first_page_on_top {
        8 - page
    } else {
        page + 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_buttons_both_ways() {
        for on_top in [true, false] {
            for page in 0..8 {
                let n = button_of_page(page, on_top).unwrap();
                assert_eq!(page_of_button(n, on_top), page);
            }
            assert_eq!(button_of_page(8, on_top), None);
        }
        // UniPad's way: the first page at the top.
        assert_eq!(button_of_page(0, true), Some(8));
    }
}
