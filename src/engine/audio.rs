//! The sounds played: a mixer of voices on the audio thread, told what to
//! play through a ring buffer, and the backend that runs it.
//!
//! The audio thread never waits on a lock, never allocates and never frees:
//! its voices are made room for once, every sound is in memory, decoded at
//! the output's rate, before it can be played, and a bank of sounds it lets
//! go of is handed back to be freed elsewhere.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

/// How many sounds play at once. One more fades out the one playing
/// longest.
pub const MAX_VOICES: usize = 64;
/// Room for the sounds fading out beside them, which are cut short only
/// when even this is full.
const FADING_ROOM: usize = 32;
/// The loudest the output goes: louder sums are turned down, not clipped.
const CEILING: f32 = 0.98;
/// How fast the limiter lets the level back up once a peak has passed.
const RELEASE_SECONDS: f32 = 0.15;
/// The sounds are stereo, interleaved.
pub const CHANNELS: usize = 2;
/// How long a stopped sound takes to fade out, so that it does not click.
const FADE_SECONDS: f32 = 0.005;
/// How many commands may wait for the audio thread.
const COMMANDS: usize = 1024;

/// Every sound of a cover, decoded.
pub struct Bank {
    /// The rate they were decoded at, which must be the output's.
    pub rate: u32,
    /// Stereo, interleaved, by their place in the project's samples.
    pub samples: Vec<Box<[f32]>>,
}

/// What the audio thread is told.
pub enum Command {
    /// Play these sounds from now on; every voice stops.
    SetBank(Arc<Bank>),
    Play {
        sample: u32,
        gain: f32,
        /// Times through, or 0 for no end.
        passes: u32,
        /// What it can be stopped by, with [`Command::StopTag`].
        tag: u32,
        /// Its choke group, or 0 for none.
        choke: u8,
    },
    StopTag(u32),
    /// Fade out what `tag` plays over so many frames, as a sampler's
    /// release does.
    Release {
        tag: u32,
        frames: u32,
    },
    Choke(u8),
    StopAll,
}

#[derive(Clone, Copy)]
struct Voice {
    sample: u32,
    /// The next frame to play.
    frame: usize,
    /// Times through still to go, this one included, or 0 for no end.
    passes: u32,
    gain: f32,
    tag: u32,
    choke: u8,
    /// When it started, to know which one is the oldest.
    started: u64,
    /// Frames of its fade still to go, and of the whole fade, once stopped.
    fading: Option<(u32, u32)>,
}

/// What mixes the voices, on the audio thread.
pub struct Mixer {
    bank: Option<Arc<Bank>>,
    voices: Vec<Voice>,
    commands: rtrb::Consumer<Command>,
    /// Banks let go of, to be freed off the audio thread.
    garbage: rtrb::Producer<Arc<Bank>>,
    started: u64,
    fade_frames: u32,
    /// The limiter's hold on the level: the peak it last met, falling.
    envelope: f32,
    /// How much the envelope keeps of itself from a frame to the next.
    release: f32,
}

/// What tells the mixer what to play, from any thread but the audio one.
pub struct AudioControl {
    commands: rtrb::Producer<Command>,
    garbage: rtrb::Consumer<Arc<Bank>>,
    rate: u32,
}

impl AudioControl {
    /// The rate sounds are to be decoded at.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn send(&mut self, command: Command) {
        // The banks the mixer let go of, freed here.
        while self.garbage.pop().is_ok() {}
        if self.commands.push(command).is_err() {
            log::warn!("the audio thread is behind: a command was dropped");
        }
    }
}

/// A mixer at a rate, and what controls it.
pub fn mixer(rate: u32) -> (AudioControl, Mixer) {
    let (commands_tx, commands_rx) = rtrb::RingBuffer::new(COMMANDS);
    let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(16);
    let mixer = Mixer {
        bank: None,
        voices: Vec::with_capacity(MAX_VOICES + FADING_ROOM),
        commands: commands_rx,
        garbage: garbage_tx,
        started: 0,
        fade_frames: ((rate as f32 * FADE_SECONDS) as u32).max(1),
        envelope: 0.0,
        release: (-1.0 / (rate as f32 * RELEASE_SECONDS)).exp(),
    };
    (
        AudioControl {
            commands: commands_tx,
            garbage: garbage_rx,
            rate,
        },
        mixer,
    )
}

impl Mixer {
    /// Fill `out`, stereo and interleaved, with what is playing.
    pub fn process(&mut self, out: &mut [f32]) {
        while let Ok(command) = self.commands.pop() {
            self.apply(command);
        }
        out.fill(0.0);
        let Some(bank) = &self.bank else { return };
        let frames = out.len() / CHANNELS;
        let mut i = 0;
        while i < self.voices.len() {
            let voice = &mut self.voices[i];
            let data = &bank.samples[voice.sample as usize];
            let length = data.len() / CHANNELS;
            let mut done = length == 0;
            for f in 0..frames {
                if done {
                    break;
                }
                if voice.frame >= length {
                    if voice.passes == 1 {
                        done = true;
                        break;
                    }
                    voice.passes = voice.passes.saturating_sub(1);
                    voice.frame = 0;
                }
                let mut gain = voice.gain;
                if let Some((left, total)) = &mut voice.fading {
                    gain *= *left as f32 / *total as f32;
                    *left = left.saturating_sub(1);
                    done = *left == 0;
                }
                out[f * CHANNELS] += data[voice.frame * CHANNELS] * gain;
                out[f * CHANNELS + 1] += data[voice.frame * CHANNELS + 1] * gain;
                voice.frame += 1;
            }
            if done {
                self.voices.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.limit(out);
    }

    /// Turn the output down where the voices together would go past the
    /// ceiling, at once on the way up and smoothly back: a clipped wave is
    /// the crackle of sounds piling up.
    fn limit(&mut self, out: &mut [f32]) {
        for frame in out.as_chunks_mut::<CHANNELS>().0 {
            let peak = frame
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            self.envelope = peak.max(self.envelope * self.release);
            if self.envelope > CEILING {
                let gain = CEILING / self.envelope;
                frame.iter_mut().for_each(|sample| *sample *= gain);
            }
        }
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::SetBank(bank) => {
                self.voices.clear();
                if let Some(old) = self.bank.replace(bank)
                    && let Err(rtrb::PushError::Full(old)) = self.garbage.push(old)
                {
                    // Not freed here, whatever it costs: the controlling
                    // side empties the garbage at every command, so this
                    // takes sixteen banks in a row without one.
                    std::mem::forget(old);
                }
            }
            Command::Play {
                sample,
                gain,
                passes,
                tag,
                choke,
            } => {
                let Some(bank) = &self.bank else { return };
                if sample as usize >= bank.samples.len() {
                    return;
                }
                if choke != 0 {
                    self.fade(self.fade_frames, |voice| voice.choke == choke);
                }
                // The oldest sound playing makes room by fading out; where
                // even the room for fading is full, the one nearest its end
                // goes at once.
                let playing = self.voices.iter().filter(|v| v.fading.is_none()).count();
                if playing >= MAX_VOICES
                    && let Some(oldest) = (0..self.voices.len())
                        .filter(|&i| self.voices[i].fading.is_none())
                        .min_by_key(|&i| self.voices[i].started)
                {
                    self.voices[oldest].fading = Some((self.fade_frames, self.fade_frames));
                }
                if self.voices.len() >= MAX_VOICES + FADING_ROOM
                    && let Some(nearest) =
                        (0..self.voices.len()).min_by_key(|&i| match self.voices[i].fading {
                            Some((left, _)) => left,
                            None => u32::MAX,
                        })
                {
                    self.voices.swap_remove(nearest);
                }
                self.started += 1;
                self.voices.push(Voice {
                    sample,
                    frame: 0,
                    passes,
                    gain,
                    tag,
                    choke,
                    started: self.started,
                    fading: None,
                });
            }
            Command::StopTag(tag) => self.fade(self.fade_frames, |voice| voice.tag == tag),
            // Never quicker than a stop, so that it does not click.
            Command::Release { tag, frames } => {
                self.fade(frames.max(self.fade_frames), |voice| voice.tag == tag)
            }
            Command::Choke(group) => self.fade(self.fade_frames, |voice| voice.choke == group),
            Command::StopAll => self.fade(self.fade_frames, |_| true),
        }
    }

    /// Fade out the voices `which` picks over `frames`, or sooner if they
    /// were already fading.
    fn fade(&mut self, frames: u32, which: impl Fn(&Voice) -> bool) {
        for voice in &mut self.voices {
            if !which(voice) {
                continue;
            }
            voice.fading = match voice.fading {
                Some((left, total)) if left <= frames => Some((left, total)),
                _ => Some((frames, frames)),
            };
        }
    }
}

/// What plays the mixer out.
pub trait AudioBackend {
    fn sample_rate(&self) -> u32;
    /// What it plays to, for the user.
    fn description(&self) -> &str;
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AudioError(String);

/// The mixer played through cpal: PipeWire when it is there, ALSA if not.
pub struct CpalBackend {
    _stream: cpal::Stream,
    rate: u32,
    description: String,
}

impl AudioBackend for CpalBackend {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl CpalBackend {
    /// Start playing to the default output, `buffer_frames` at a time if it
    /// takes that, as it likes if not.
    pub fn start(buffer_frames: u32) -> Result<(Self, AudioControl), AudioError> {
        let mut hosts = vec![cpal::default_host()];
        if hosts[0].id() != cpal::HostId::Alsa
            && let Ok(alsa) = cpal::host_from_id(cpal::HostId::Alsa)
        {
            hosts.push(alsa);
        }
        let mut last = AudioError("no audio output".into());
        for host in hosts {
            for fixed in [true, false] {
                match Self::start_on(&host, fixed.then_some(buffer_frames)) {
                    Ok(started) => return Ok(started),
                    Err(e) => {
                        log::warn!("no audio through {:?}: {e}", host.id());
                        last = e;
                    }
                }
            }
        }
        Err(last)
    }

    fn start_on(
        host: &cpal::Host,
        buffer_frames: Option<u32>,
    ) -> Result<(Self, AudioControl), AudioError> {
        let error = |e: cpal::Error| AudioError(e.to_string());
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioError("no audio output".into()))?;
        let supported = device.default_output_config().map_err(error)?;
        let format = supported.sample_format();
        let mut config = supported.config();
        if let Some(frames) = buffer_frames {
            config.buffer_size = cpal::BufferSize::Fixed(frames);
        }
        let rate = config.sample_rate;
        let (control, mixer) = mixer(rate);
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, mixer),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, mixer),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, mixer),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, mixer),
            cpal::SampleFormat::F64 => build::<f64>(&device, config, mixer),
            other => {
                return Err(AudioError(format!(
                    "the output takes {other} samples, which PadWire does not write"
                )));
            }
        }
        .map_err(error)?;
        stream.play().map_err(error)?;
        let name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "the default output".into());
        let buffer = match buffer_frames {
            Some(frames) => format!("{frames} frames"),
            None => "its own buffer".into(),
        };
        let description = format!("{name} at {rate} Hz, {buffer}, through {:?}", host.id());
        log::info!("playing to {description}");
        Ok((
            CpalBackend {
                _stream: stream,
                rate,
                description,
            },
            control,
        ))
    }
}

/// How the audio thread was made real-time.
enum Realtime {
    /// By the kernel, as the user's limits allow: `realtime` group members'
    /// do, which is how PipeWire's own threads are.
    Fifo(i32),
    /// By rtkit, which keeps it so while the handle lives.
    Rtkit(#[allow(dead_code)] audio_thread_priority::RtPriorityHandle),
}

impl Realtime {
    fn describe(&self) -> String {
        match self {
            Realtime::Fifo(priority) => format!("at priority {priority}"),
            Realtime::Rtkit(_) => "through rtkit".into(),
        }
    }
}

/// Below PipeWire's own threads (88), above everything else.
const REALTIME_PRIORITY: i32 = 80;

/// Make the calling thread real-time: by the kernel where the limits
/// allow, through rtkit if not.
fn make_realtime(frames: u32, rate: u32) -> Result<Realtime, String> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is valid for the call to fill.
    let allowed = unsafe { libc::getrlimit(libc::RLIMIT_RTPRIO, &mut limit) } == 0;
    let priority = REALTIME_PRIORITY.min(limit.rlim_cur.min(99) as i32);
    if allowed && priority > 0 {
        let param = libc::sched_param {
            sched_priority: priority,
        };
        // SAFETY: the calling thread, a valid policy and a valid parameter.
        let set =
            unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
        if set == 0 {
            return Ok(Realtime::Fifo(priority));
        }
    }
    audio_thread_priority::promote_current_thread_to_real_time(frames, rate)
        .map(Realtime::Rtkit)
        .map_err(|e| e.to_string())
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut mixer: Mixer,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    // Made room for once: the callback never asks for more than this at a
    // time, larger buffers being mixed a part at a time.
    const CHUNK: usize = 4096;
    let mut scratch = vec![0.0f32; CHUNK * CHANNELS];
    let rate = config.sample_rate;
    // The thread the callback runs on, made real-time on its first call:
    // without it, any load on the computer can keep it waiting past its
    // turn, and the sound crackles. The handle keeps it so, under rtkit.
    let mut realtime: Option<Result<Realtime, String>> = None;
    device.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _| {
            if realtime.is_none() {
                let frames = (out.len() / channels.max(1)) as u32;
                let promoted = make_realtime(frames, rate);
                // Once, at the start: the only time the audio thread logs.
                match &promoted {
                    Ok(how) => log::info!("the audio thread runs in real time, {}", how.describe()),
                    Err(e) => log::warn!(
                        "the audio thread cannot run in real time, and may crackle under load: {e}"
                    ),
                }
                realtime = Some(promoted);
            }
            for part in out.chunks_mut(CHUNK * channels) {
                let frames = part.len() / channels;
                let mixed = &mut scratch[..frames * CHANNELS];
                mixer.process(mixed);
                for (frame, stereo) in part
                    .chunks_exact_mut(channels)
                    .zip(mixed.as_chunks::<CHANNELS>().0)
                {
                    match channels {
                        1 => frame[0] = T::from_sample((stereo[0] + stereo[1]) * 0.5),
                        _ => {
                            frame[0] = T::from_sample(stereo[0]);
                            frame[1] = T::from_sample(stereo[1]);
                            for rest in &mut frame[2..] {
                                *rest = T::from_sample(0.0);
                            }
                        }
                    }
                }
            }
        },
        |e| log::warn!("audio: {e}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank(samples: Vec<Vec<f32>>) -> Arc<Bank> {
        Arc::new(Bank {
            rate: 1000,
            samples: samples.into_iter().map(Vec::into_boxed_slice).collect(),
        })
    }

    fn play(sample: u32, passes: u32, tag: u32) -> Command {
        Command::Play {
            sample,
            gain: 1.0,
            passes,
            tag,
            choke: 0,
        }
    }

    #[test]
    fn plays_through_then_stops() {
        let (mut control, mut mixer) = mixer(1000);
        control.send(Command::SetBank(bank(vec![vec![0.5, -0.5, 0.25, -0.25]])));
        control.send(play(0, 1, 1));
        let mut out = [0.0; 6];
        mixer.process(&mut out);
        assert_eq!(out, [0.5, -0.5, 0.25, -0.25, 0.0, 0.0]);
        assert!(mixer.voices.is_empty());
    }

    #[test]
    fn loops_as_many_times_as_told() {
        let (mut control, mut mixer) = mixer(1000);
        control.send(Command::SetBank(bank(vec![vec![0.5, 0.5]])));
        control.send(play(0, 3, 1));
        let mut out = [0.0; 10];
        mixer.process(&mut out);
        assert_eq!(out, [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn stopping_fades_out() {
        // 5 frames of fade at 1000 Hz.
        let (mut control, mut mixer) = mixer(1000);
        control.send(Command::SetBank(bank(vec![vec![0.5; 200]])));
        control.send(play(0, 0, 7));
        let mut out = [0.0; 4];
        mixer.process(&mut out);
        control.send(Command::StopTag(7));
        let mut out = [0.0; 20];
        mixer.process(&mut out);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let expected = [0.5, 0.4, 0.3, 0.2, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0];
        for (got, want) in left.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{left:?}");
        }
        assert!(mixer.voices.is_empty());
    }

    #[test]
    fn the_oldest_voice_makes_room() {
        let (mut control, mut mixer) = mixer(1000);
        control.send(Command::SetBank(bank(vec![vec![0.0; 1000]])));
        for tag in 0..=MAX_VOICES as u32 {
            control.send(play(0, 0, tag));
        }
        // The first fades out rather than stopping dead…
        mixer.process(&mut [0.0; 2]);
        assert_eq!(mixer.voices.len(), MAX_VOICES + 1);
        let first = mixer.voices.iter().find(|v| v.tag == 0).unwrap();
        assert!(first.fading.is_some());
        // …and is gone once faded, 5 ms at 1000 Hz.
        mixer.process(&mut [0.0; 10]);
        assert_eq!(mixer.voices.len(), MAX_VOICES);
        assert!(mixer.voices.iter().all(|v| v.tag != 0));
    }

    #[test]
    fn loud_sums_are_turned_down_not_clipped() {
        let (mut control, mut mixer) = mixer(1000);
        control.send(Command::SetBank(bank(vec![vec![0.8; 20]])));
        // Three at 0.8: 2.4 together.
        for tag in 0..3 {
            control.send(play(0, 1, tag));
        }
        let mut out = [0.0; 20];
        mixer.process(&mut out);
        for sample in out {
            assert!((sample - CEILING).abs() < 1e-6, "{out:?}");
        }
    }
}
