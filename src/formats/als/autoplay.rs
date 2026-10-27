//! An Ableton cover played through: a clip of the pads' notes, which its
//! maker recorded or wrote.
//!
//! Two kinds are known. Kaskobi's covers hold a clip named "Tutorial", the
//! page buttons among its notes. The Faded cover holds clips of the whole
//! song on its track of sounds, the page turned by an envelope of the
//! control the page buttons are mapped to. The first found is taken: a clip
//! named for it, else the clip of the sounds that turns the pages and has
//! the most notes.

use super::PageMapping;
use super::xml::Node;
use crate::device::layout::drum_rack;
use crate::model::{AutoAction, AutoEvent, AutoPlay};

/// What the clip is read with.
pub struct Context<'a> {
    pub sounds: &'a Node,
    pub mapping: Option<&'a PageMapping>,
    /// The id Live automates the page control by.
    pub page_target: Option<&'a str>,
    /// The note of the first page's button.
    pub page_notes: u8,
    pub pages: usize,
    pub tempo: f32,
}

/// A clip, with the track it is on.
struct Clip<'a> {
    node: &'a Node,
    track: &'a Node,
}

pub fn read(set: &Node, context: &Context) -> Option<AutoPlay> {
    let mut clips = Vec::new();
    for track in set
        .child("Tracks")
        .into_iter()
        .flat_map(|tracks| tracks.children("MidiTrack"))
    {
        let mut found = Vec::new();
        if let Some(sequencer) = track.at("DeviceChain/MainSequencer") {
            sequencer.find_all("MidiClip", &mut found);
        }
        clips.extend(found.into_iter().map(|node| Clip { node, track }));
    }
    let named = |clip: &Clip| {
        let name = |node: &Node, path: &str| node.value_at(path).unwrap_or_default().to_lowercase();
        let names = [
            name(clip.node, "Name"),
            name(clip.track, "Name/EffectiveName"),
        ];
        names
            .iter()
            .any(|name| name.contains("tutorial") || name.contains("autoplay"))
    };
    // A clip named for it, the one with page buttons in it first.
    let mut tutorials: Vec<&Clip> = clips.iter().filter(|clip| named(clip)).collect();
    tutorials.sort_by_key(|clip| {
        let notes = notes(clip.node);
        let pages = notes
            .iter()
            .filter(|n| page_of_note(n.key, context).is_some())
            .count();
        (std::cmp::Reverse(pages > 0), std::cmp::Reverse(notes.len()))
    });
    if let Some(clip) = tutorials.first() {
        return Some(perform(clip.node, context));
    }
    // A clip of the sounds that turns the pages.
    let target = context.page_target?;
    clips
        .iter()
        .filter(|clip| std::ptr::eq(clip.track, context.sounds))
        .filter(|clip| page_envelope(clip.node, target).is_some())
        .max_by_key(|clip| notes(clip.node).len())
        .map(|clip| perform(clip.node, context))
}

/// A note of a clip, in beats.
struct ClipNote {
    time: f64,
    duration: f64,
    key: u8,
}

fn notes(clip: &Node) -> Vec<ClipNote> {
    let mut notes = Vec::new();
    let Some(tracks) = clip.at("Notes/KeyTracks") else {
        return notes;
    };
    for track in tracks.children("KeyTrack") {
        let Some(key) = track.parse_at::<u8>("MidiKey") else {
            continue;
        };
        let Some(events) = track.child("Notes") else {
            continue;
        };
        for event in events.children("MidiNoteEvent") {
            if event.attr("IsEnabled") == Some("false") {
                continue;
            }
            let number = |name: &str| event.attr(name).and_then(|v| v.parse::<f64>().ok());
            if let (Some(time), Some(duration)) = (number("Time"), number("Duration")) {
                notes.push(ClipNote {
                    time,
                    duration,
                    key,
                });
            }
        }
    }
    notes
}

/// The page a note's button turns to.
fn page_of_note(key: u8, context: &Context) -> Option<usize> {
    let page = usize::from(key.checked_sub(context.page_notes)?);
    (page < context.pages.max(1)).then_some(page)
}

/// The values the page control takes in a clip, by time in beats.
fn page_envelope(clip: &Node, target: &str) -> Option<Vec<(f64, f32)>> {
    let envelopes = clip.at("Envelopes/Envelopes")?;
    let envelope = envelopes
        .children("ClipEnvelope")
        .find(|envelope| envelope.value_at("EnvelopeTarget/PointeeId") == Some(target))?;
    let events = envelope.at("Automation/Events")?;
    let points: Vec<(f64, f32)> = events
        .children("FloatEvent")
        .filter_map(|event| {
            Some((
                event.attr("Time")?.parse().ok()?,
                event.attr("Value")?.parse().ok()?,
            ))
        })
        .collect();
    (!points.is_empty()).then_some(points)
}

/// The clip as presses, let gos and page turns, in milliseconds.
fn perform(clip: &Node, context: &Context) -> AutoPlay {
    let beat_ms = 60_000.0 / f64::from(context.tempo.max(1.0));
    let ms = |beats: f64| beats.max(0.0) * beat_ms;
    let mut events = Vec::new();
    for note in notes(clip) {
        if let Some((x, y)) = drum_rack::pad(note.key) {
            events.push(AutoEvent {
                at_ms: ms(note.time),
                action: AutoAction::Press(x, y),
            });
            events.push(AutoEvent {
                at_ms: ms(note.time + note.duration),
                action: AutoAction::Release(x, y),
            });
        } else if let Some(page) = page_of_note(note.key, context) {
            events.push(AutoEvent {
                at_ms: ms(note.time),
                action: AutoAction::Page {
                    page,
                    restart_sounds: false,
                },
            });
        }
    }
    // The page control's envelope, where the clip has one: its value as the
    // page whose button sets it nearest.
    if let (Some(target), Some(mapping)) = (context.page_target, context.mapping)
        && let Some(points) = page_envelope(clip, target)
    {
        let mut last = None;
        for (time, value) in points {
            let page = (0..mapping.pages())
                .min_by(|&a, &b| {
                    let off = |page: usize| (f32::from(mapping.value(page)) - value).abs();
                    off(a).total_cmp(&off(b))
                })
                .unwrap_or(0);
            if last != Some(page) {
                events.push(AutoEvent {
                    at_ms: ms(time),
                    action: AutoAction::Page {
                        page,
                        restart_sounds: false,
                    },
                });
                last = Some(page);
            }
        }
    }
    // In time; at the same time, the page first, then the let gos, then the
    // presses.
    let order = |action: &AutoAction| match action {
        AutoAction::Page { .. } => 0,
        AutoAction::Release(..) => 1,
        AutoAction::Press(..) => 2,
    };
    events.sort_by(|a, b| {
        a.at_ms
            .total_cmp(&b.at_ms)
            .then(order(&a.action).cmp(&order(&b.action)))
    });
    AutoPlay { events }
}
