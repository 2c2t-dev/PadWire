//! A cover's sounds decoded into memory, at the output's rate, before it is
//! played: nothing is read from the disk while the pads are hit.

use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use super::audio::{Bank, CHANNELS};
use crate::formats::FileSource;
use crate::model::SampleFile;

/// Every sound of a cover, decoded on as many threads as there are cores.
/// `progress` is told how many are done out of how many, from any of them.
///
/// A sound that cannot be read is silent, and said so in what comes back
/// beside the bank.
pub fn decode_all(
    files: &dyn FileSource,
    samples: &[SampleFile],
    rate: u32,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> (Bank, Vec<String>) {
    let decoded: Vec<Mutex<Option<Box<[f32]>>>> =
        samples.iter().map(|_| Mutex::new(None)).collect();
    let failures = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(samples.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sample) = samples.get(i) else { break };
                    let result = files
                        .read(&sample.path)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| {
                            decode(bytes, extension(&sample.path), sample.range, rate)
                        });
                    let audio = result.unwrap_or_else(|e| {
                        failures
                            .lock()
                            .unwrap()
                            .push(format!("{}: {e}", sample.path));
                        Box::default()
                    });
                    *decoded[i].lock().unwrap() = Some(audio);
                    progress(done.fetch_add(1, Ordering::Relaxed) + 1, samples.len());
                }
            });
        }
    });
    let samples = decoded
        .into_iter()
        .map(|slot| slot.into_inner().unwrap().unwrap_or_default())
        .collect();
    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    (Bank { rate, samples }, failures)
}

fn extension(path: &str) -> Option<&str> {
    path.rsplit_once('.').map(|(_, extension)| extension)
}

/// A sound as stereo at `rate`: whole, or the frames of `range`, first and
/// last, counted at its own rate.
pub fn decode(
    bytes: Vec<u8>,
    extension: Option<&str>,
    range: Option<(u64, u64)>,
    rate: u32,
) -> Result<Box<[f32]>, String> {
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = extension {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("not a sound PadWire reads ({e})"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("holds no sound")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or("holds no sound")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("in a codec PadWire does not decode ({e})"))?;

    let mut stereo = Vec::new();
    let mut packet_samples = Vec::new();
    let mut source_rate = None;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // What could be read of a damaged file is kept.
            Err(e) => {
                log::debug!("a sound stops short: {e}");
                break;
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(audio) => audio,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => {
                log::debug!("a sound stops short: {e}");
                break;
            }
        };
        let channels = audio.spec().channels().count();
        source_rate.get_or_insert(audio.spec().rate());
        packet_samples.resize(audio.samples_interleaved(), 0.0f32);
        audio.copy_to_slice_interleaved(&mut packet_samples);
        to_stereo(&packet_samples, channels, &mut stereo);
    }
    let source_rate = source_rate.ok_or("holds no sound")?;
    if let Some((first, last)) = range {
        let frames = (stereo.len() / CHANNELS) as u64;
        let end = last.saturating_add(1).min(frames);
        let start = first.min(end);
        stereo.truncate(end as usize * CHANNELS);
        stereo.drain(..start as usize * CHANNELS);
    }
    Ok(resample(&stereo, source_rate, rate).into_boxed_slice())
}

/// Interleaved samples of any number of channels added as stereo: one
/// channel is heard on both sides, and past two only the first two are kept.
fn to_stereo(samples: &[f32], channels: usize, into: &mut Vec<f32>) {
    match channels {
        0 => {}
        1 => into.extend(samples.iter().flat_map(|&s| [s, s])),
        _ => into.extend(
            samples
                .chunks_exact(channels)
                .flat_map(|frame| [frame[0], frame[1]]),
        ),
    }
}

/// Stereo from one rate to another, by cubic interpolation: plenty for
/// 44.1 kHz to 48 kHz, which is what covers mostly need. Going down a long
/// way would want a filter first.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let frames = input.len() / CHANNELS;
    let step = f64::from(from) / f64::from(to);
    let out_frames = ((frames as f64) / step).floor() as usize;
    let at = |frame: isize, channel: usize| -> f32 {
        let frame = frame.clamp(0, frames as isize - 1) as usize;
        input[frame * CHANNELS + channel]
    };
    let mut out = Vec::with_capacity(out_frames * CHANNELS);
    for i in 0..out_frames {
        let position = i as f64 * step;
        let base = position.floor() as isize;
        let t = (position - base as f64) as f32;
        for channel in 0..CHANNELS {
            let (y0, y1, y2, y3) = (
                at(base - 1, channel),
                at(base, channel),
                at(base + 1, channel),
                at(base + 2, channel),
            );
            // Catmull-Rom.
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            out.push(((a * t + b) * t + c) * t + y1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_untouched() {
        let input = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(resample(&input, 48_000, 48_000), input);
    }

    #[test]
    fn resampled_length_and_shape() {
        // A ramp stays a ramp, away from its start, which is held.
        let input: Vec<f32> = (0..441).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let out = resample(&input, 44_100, 48_000);
        assert_eq!(out.len() / 2, 480);
        let step = 44_100.0 / 48_000.0;
        for i in 3..470 {
            assert!((out[i * 2] - i as f32 * step).abs() < 1e-3, "frame {i}");
            assert!((out[i * 2 + 1] + i as f32 * step).abs() < 1e-3);
        }
    }

    #[test]
    fn mono_is_heard_on_both_sides() {
        let mut out = Vec::new();
        to_stereo(&[0.5, 0.25], 1, &mut out);
        to_stereo(&[1.0, 2.0, 3.0], 3, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.25, 0.25, 1.0, 2.0]);
    }
}
