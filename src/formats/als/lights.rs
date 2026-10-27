//! The lights of an Ableton cover: tracks of MIDI effects that take the
//! notes of the pads and send theirs to the Launchpad.
//!
//! What PadWire plays of them is what the covers it was made with use:
//! Pitch, Chord, Velocity, Note Length, Arpeggiator, and racks of them
//! chosen by key, velocity and Chain Selector, as in the Faded cover; and,
//! from Kaskobi's Max for Live devices, the MIDI files his MIDI Launcher
//! plays, and the page his Page Switcher sets. A device it does not know
//! lets notes through unchanged, and is counted in a warning; Max for Live
//! devices are closed programs, which PadWire cannot play but for those.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::paths::{FileRef, Resolver};
use super::xml::Node;
use crate::model::midifx::{
    ArpStyle, Arpeggiator, Branch, Chain, Device, FileNote, MidiFile, MidiLights, Rack, Resonator,
    Selector, Shape, VelocityMap, VelocityMode, max_rate_beats, synced_rate_beats,
};

/// The note of the first page's button when no mapping says.
const PAGE_NOTES: u8 = 100;

/// The tracks of lights of a set: MIDI tracks that send to a device, or to
/// another track that does, other than the one of the sounds. `None` when it
/// has none. The MIDI files they play are looked for as samples are.
pub fn read(
    set: &Node,
    sounds: &Node,
    page_notes: Option<u8>,
    resolver: &Resolver,
    bases: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Option<MidiLights> {
    let tempo = set
        .parse_at::<f32>("MasterTrack/DeviceChain/Mixer/Tempo/Manual")
        .filter(|t| *t > 0.0)
        .unwrap_or(120.0);
    let mut reader = Reader {
        tempo,
        resolver,
        bases,
        unknown: BTreeMap::new(),
        passed: BTreeMap::new(),
        unsupported: BTreeMap::new(),
        missing: Vec::new(),
    };
    let mut chains = Vec::new();
    for track in set
        .child("Tracks")
        .into_iter()
        .flat_map(|tracks| tracks.children("MidiTrack"))
    {
        if std::ptr::eq(track, sounds) {
            continue;
        }
        // A track that hears nothing, as a tutorial's, which plays clips of
        // its own, makes no lights of the pads.
        if track.value_at("DeviceChain/MidiInputRouting/Target") == Some("MidiIn/None") {
            continue;
        }
        let to_device = track
            .value_at("DeviceChain/MidiOutputRouting/Target")
            .is_some_and(|target| {
                target.starts_with("MidiOut/External") || target.starts_with("MidiOut/Track.")
            });
        let Some(devices) = track.at("DeviceChain/DeviceChain/Devices") else {
            continue;
        };
        if !to_device {
            continue;
        }
        let chain = reader.chain(devices);
        if !chain.is_empty() {
            chains.push(chain);
        }
    }
    for (device, count) in &reader.unknown {
        let what = if device.starts_with("Max for Live") {
            "make lights of their own, which PadWire cannot"
        } else {
            "are not played: notes go through them unchanged"
        };
        warnings.push(format!("{count} {device} device(s) of the lights {what}"));
    }
    for (device, count) in &reader.passed {
        warnings.push(format!(
            "{count} {device} device(s) of the lights are taken as passing notes on unchanged"
        ));
    }
    if !reader.missing.is_empty() {
        reader.missing.sort();
        reader.missing.dedup();
        warnings.push(format!(
            "{} MIDI file(s) of the lights not found: {}",
            reader.missing.len(),
            reader.missing.join(", ")
        ));
    }
    for (setting, count) in &reader.unsupported {
        warnings.push(format!(
            "{count} device(s) of the lights use {setting}, which PadWire does not play"
        ));
    }
    let palette = video_palette(set).map(Arc::new);
    (!chains.is_empty()).then_some(MidiLights {
        palette,
        chains,
        tempo,
        page_notes: page_notes.unwrap_or(PAGE_NOTES),
    })
}

struct Reader<'a> {
    tempo: f32,
    resolver: &'a Resolver,
    bases: &'a [PathBuf],
    unknown: BTreeMap<String, usize>,
    /// Max for Live devices taken as letting notes through.
    passed: BTreeMap<String, usize>,
    unsupported: BTreeMap<&'static str, usize>,
    missing: Vec<String>,
}

/// The name of a Max for Live device: its file's, without `.amxd`.
fn max_device_name(device: &Node) -> Option<String> {
    let mut refs = Vec::new();
    device.at("PatchSlot")?.find_all("FileRef", &mut refs);
    let name = refs
        .iter()
        .find_map(|r| FileRef::read(r)?.name().map(str::to_owned))?;
    Some(name.trim_end_matches(".amxd").to_owned())
}

/// What a Max for Live device keeps of itself in the set, beside its
/// parameters: a JSON object, written out in hex.
fn saved_state(device: &Node) -> Option<serde_json::Map<String, serde_json::Value>> {
    let hex: String = device
        .at("BlobSlot/Value/MxDBlob/Blob")?
        .text()?
        .split_whitespace()
        .collect();
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    serde_json::from_slice(bytes).ok()
}

/// The palette a cover's lights were made with, when it holds Kaskobi's
/// Video Palette: the device shows a chart of the colours of the
/// velocities, and keeps them in the set, a table for each of red, green
/// and blue, 0 to 63. The tables are told apart by the colours every RGB
/// Launchpad palette has: 5 red, 21 green, 45 blue.
fn video_palette(set: &Node) -> Option<Vec<[u8; 3]>> {
    let mut devices = Vec::new();
    set.find_all("MxDeviceMidiEffect", &mut devices);
    let device = devices.into_iter().find(|device| {
        max_device_name(device).is_some_and(|name| name.contains("Video Palette"))
    })?;
    let state = saved_state(device)?;
    let tables: Vec<Vec<u8>> = state
        .values()
        .filter_map(|value| {
            let table: Vec<u8> = value
                .as_array()?
                .iter()
                .map(|level| level.as_u64().map(|level| level.min(63) as u8))
                .collect::<Option<_>>()?;
            (table.len() == 128).then_some(table)
        })
        .collect();
    // The one table bright on this velocity, dark on the other two.
    let lit_on = |velocity: usize| {
        let others = [5, 21, 45].into_iter().filter(|&v| v != velocity);
        tables
            .iter()
            .find(|table| table[velocity] >= 48 && others.clone().all(|other| table[other] < 16))
    };
    let (red, green, blue) = (lit_on(5)?, lit_on(21)?, lit_on(45)?);
    Some((0..128).map(|v| [red[v], green[v], blue[v]]).collect())
}

/// The notes of a MIDI file, every track's, in beats.
fn read_midi_file(bytes: &[u8]) -> Result<MidiFile, String> {
    let smf = midly::Smf::parse(bytes).map_err(|e| e.to_string())?;
    let midly::Timing::Metrical(ticks) = smf.header.timing else {
        return Err("its time is in frames, not beats".into());
    };
    let ticks = f64::from(u16::from(ticks).max(1));
    let mut notes = Vec::new();
    for track in &smf.tracks {
        let mut tick = 0u64;
        for event in track {
            tick += u64::from(u32::from(event.delta));
            let midly::TrackEventKind::Midi { message, .. } = event.kind else {
                continue;
            };
            let (note, velocity) = match message {
                midly::MidiMessage::NoteOn { key, vel } => (u8::from(key), u8::from(vel)),
                midly::MidiMessage::NoteOff { key, .. } => (u8::from(key), 0),
                _ => continue,
            };
            notes.push(FileNote {
                beat: tick as f64 / ticks,
                note,
                velocity,
            });
        }
    }
    // In time; at the same time, ends before starts, so that a note ended
    // and started again at once stays lit.
    notes.sort_by(|a, b| {
        a.beat
            .total_cmp(&b.beat)
            .then((a.velocity > 0).cmp(&(b.velocity > 0)))
    });
    Ok(MidiFile { notes })
}

/// A device's parameter as it stands.
fn manual<'a>(device: &'a Node, parameter: &str) -> Option<&'a str> {
    device
        .child(parameter)
        .and_then(|p| p.value_at("Manual").or(p.value()))
}

fn number(device: &Node, parameter: &str) -> Option<f32> {
    manual(device, parameter).and_then(|value| match value {
        "true" => Some(1.0),
        "false" => Some(0.0),
        value => value.parse().ok(),
    })
}

fn flag(device: &Node, parameter: &str) -> bool {
    number(device, parameter).is_some_and(|value| value != 0.0)
}

fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 127.0) as u8
}

impl Reader<'_> {
    fn chain(&mut self, devices: &Node) -> Chain {
        // After a page switching device, a rack whose Chain Selector is
        // mapped to nothing follows the page.
        let mut page_switched = false;
        let mut chain = Vec::new();
        for device in &devices.children {
            if &*device.tag == "MxDeviceMidiEffect"
                && max_device_name(device).is_some_and(|name| name.contains("Page Switcher"))
            {
                page_switched = true;
                continue;
            }
            if let Some(device) = self.device(device, page_switched) {
                chain.push(device);
            }
        }
        chain
    }

    fn device(&mut self, device: &Node, page_switched: bool) -> Option<Device> {
        if device.child("On").is_some() && !flag(device, "On") {
            return None;
        }
        let device = match &*device.tag {
            "MidiPitcher" => Device::Pitch {
                shift: number(device, "Pitch").unwrap_or(0.0) as i32,
                lowest: byte(number(device, "Lowest").unwrap_or(0.0)),
                range: byte(number(device, "Range").unwrap_or(127.0)),
            },
            "MidiChord" => Device::Chord {
                shifts: (1..=6)
                    .filter_map(|i| {
                        let shift = number(device, &format!("Shift{i}"))? as i32;
                        let part = number(device, &format!("Velocity{i}")).unwrap_or(1.0);
                        (shift != 0).then_some((shift, part))
                    })
                    .collect(),
            },
            "MidiVelocity" => {
                for (setting, label) in [
                    ("Drive", "a velocity curve"),
                    ("Compand", "a velocity curve"),
                    ("Random", "random velocities"),
                ] {
                    if number(device, setting).is_some_and(|v| v != 0.0) {
                        *self.unsupported.entry(label).or_default() += 1;
                    }
                }
                Device::Velocity(VelocityMap {
                    lowest: byte(number(device, "Lowest").unwrap_or(1.0)),
                    range: byte(number(device, "Range").unwrap_or(127.0)),
                    out_low: byte(number(device, "MinOut").unwrap_or(1.0)),
                    out_high: byte(number(device, "MaxOut").unwrap_or(127.0)),
                    mode: match number(device, "Mode").unwrap_or(0.0) as u8 {
                        1 => VelocityMode::Gate,
                        2 => VelocityMode::Fixed,
                        _ => VelocityMode::Clip,
                    },
                })
            }
            "MidiNoteLength" => {
                if flag(device, "SyncState") {
                    *self
                        .unsupported
                        .entry("a length synced to the tempo")
                        .or_default() += 1;
                }
                let gate = number(device, "Gate").unwrap_or(100.0) / 100.0;
                Device::NoteLength {
                    ms: number(device, "TimeLength").unwrap_or(500.0) * gate,
                    on_release: flag(device, "Mode"),
                }
            }
            "MidiArpeggiator" => self.arpeggiator(device),
            "MidiEffectGroupDevice" => Device::Rack(self.rack(device, page_switched)),
            "MxDeviceMidiEffect" => {
                let name = max_device_name(device).unwrap_or_else(|| "?".into());
                if name.contains("Depths Selector") {
                    // Its channel, its layer, and whether it is bypassed.
                    let state = saved_state(device).unwrap_or_default();
                    let number = |key: &str| {
                        state
                            .get(key)
                            .and_then(|value| value.as_array()?.first()?.as_f64())
                    };
                    if number("pictctrl").is_some_and(|bypass| bypass != 0.0) {
                        return None;
                    }
                    let layer = number("live.numbox[1]").unwrap_or(0.0);
                    return Some(Device::Layer(layer.clamp(-128.0, 127.0) as i8));
                }
                // What it mixes, layer over layer, the lights do anyway.
                if name.contains("Depths Mixer") {
                    return None;
                }
                if name.contains("Resonator") {
                    return Some(Device::Resonator(Arc::new(self.resonator(device))));
                }
                if !name.contains("MIDI Launcher") {
                    // Most route and mix the lights, and pass them on.
                    *self
                        .passed
                        .entry(format!("Max for Live “{name}”"))
                        .or_default() += 1;
                    return None;
                }
                Device::MidiFile(Arc::new(self.midi_file(device)))
            }
            other => {
                *self.unknown.entry(other.to_owned()).or_default() += 1;
                return None;
            }
        };
        Some(device)
    }

    fn arpeggiator(&mut self, device: &Node) -> Device {
        // Live's styles, by their place in its list.
        let style = match number(device, "Mode").unwrap_or(0.0) as u8 {
            0 => ArpStyle::Up,
            1 => ArpStyle::Down,
            2 => ArpStyle::UpDown,
            3 => ArpStyle::DownUp,
            _ => {
                *self
                    .unsupported
                    .entry("an arpeggio style other than up and down")
                    .or_default() += 1;
                ArpStyle::Up
            }
        };
        let step_ms = if flag(device, "SyncState") {
            let beats = synced_rate_beats(number(device, "SyncedRate").unwrap_or(6.0) as u8);
            60_000.0 / self.tempo * beats
        } else {
            number(device, "FreeRate").unwrap_or(125.0)
        };
        for (setting, label) in [
            ("Groove", "a groove"),
            ("Retrigger", "arpeggios started again"),
            ("TransposeSteps", "arpeggios transposed"),
            ("VelocitySwitch", "a velocity decay"),
        ] {
            if number(device, setting).is_some_and(|v| v != 0.0) {
                *self.unsupported.entry(label).or_default() += 1;
            }
        }
        let repeats = number(device, "RepeatCount").unwrap_or(0.0) as u32;
        Device::Arpeggiator(Arpeggiator {
            style,
            step_ms,
            gate: number(device, "Gate").unwrap_or(50.0) / 100.0,
            // 0 is Live's "inf".
            repeats: (repeats > 0).then_some(repeats),
            hold: flag(device, "Hold"),
        })
    }

    /// A Resonator, from the settings the set keeps of it: its parameters,
    /// by the names Resonator v3.0 gives them, and its direction buttons,
    /// which it keeps in its own saved state.
    fn resonator(&mut self, device: &Node) -> Resonator {
        let mut parameters: BTreeMap<String, f32> = BTreeMap::new();
        let mut found = Vec::new();
        if let Some(list) = device.at("ParameterList") {
            for kind in [
                "MxDFloatParameter",
                "MxDIntParameter",
                "MxDEnumParameter",
                "MxDBoolParameter",
            ] {
                list.find_all(kind, &mut found);
            }
        }
        for parameter in found {
            if let (Some(name), Some(value)) = (
                parameter.value_at("Name"),
                parameter.parse_at::<f32>("Timeable/Manual"),
            ) {
                parameters.insert(name.to_owned(), value);
            }
        }
        let state = saved_state(device).unwrap_or_default();
        let button = |name: &str| {
            state
                .get(name)
                .and_then(|value| value.as_array()?.first()?.as_f64())
                .is_some_and(|value| value != 0.0)
        };
        let parameter = |name: &str| parameters.get(name).copied();
        let on = |name: &str| parameter(name).is_some_and(|value| value >= 0.5);
        let beat_ms = 60_000.0 / self.tempo;
        // A time in ms or synced to the tempo, as the switch beside it says.
        // The ms knob is exponential: Live keeps where it stands, which is
        // the 10th root of how far along its range the time is.
        let time = |switch: &str, sync: &str, ms: &str| -> f32 {
            if on(switch) {
                beat_ms * max_rate_beats(parameter(sync).unwrap_or(5.0) as u8)
            } else {
                let (low, high) = (10.0f32, 60_000.0f32);
                let stored = parameter(ms).unwrap_or(31_320.0).clamp(low, high);
                low + (high - low) * ((stored - low) / (high - low)).powi(10)
            }
        };

        let directions = [
            ("pictctrl[11]", (0, 1)),
            ("pictctrl[10]", (0, -1)),
            ("pictctrl[8]", (-1, 0)),
            ("pictctrl[9]", (1, 0)),
            ("pictctrl[3]", (-1, 1)),
            ("pictctrl[5]", (1, 1)),
            ("pictctrl[6]", (-1, -1)),
            ("pictctrl[4]", (1, -1)),
        ]
        .into_iter()
        .filter(|(name, _)| button(name))
        .map(|(_, direction)| direction)
        .collect();
        let shapes = [
            ("pictctrl[12]", Shape::Square),
            ("pictctrl[13]", Shape::Circle),
            ("pictctrl[15]", Shape::Diamond),
        ]
        .into_iter()
        .filter(|(name, _)| button(name))
        .map(|(_, shape)| shape)
        .collect();
        // The gradient's colours, first to last, by their parameters' names.
        const GRADIENT: [&str; 16] = [
            "live.numbox[33]",
            "live.numbox[21]",
            "live.numbox[32]",
            "live.numbox[31]",
            "live.numbox[30]",
            "live.numbox[20]",
            "live.numbox[29]",
            "live.numbox[28]",
            "live.numbox[27]",
            "live.numbox[19]",
            "live.numbox[18]",
            "live.numbox[17]",
            "live.numbox[37]",
            "live.numbox[36]",
            "live.numbox[35]",
            "live.numbox[34]",
        ];
        let colours = (parameter("live.numbox[40]").unwrap_or(1.0) as usize).clamp(1, 16);
        let gradient = on("live.text[33]").then(|| {
            GRADIENT[..colours]
                .iter()
                .map(|name| byte(parameter(name).unwrap_or(3.0)))
                .collect()
        });
        if on("live.text[60]") {
            *self
                .unsupported
                .entry("a Resonator started on release too")
                .or_default() += 1;
        }
        Resonator {
            directions,
            shapes,
            steps: byte(parameter("live.numbox").unwrap_or(10.0)),
            move_ms: time("live.text[40]", "live.numbox[41]", "live.numbox[42]"),
            inwards: on("live.text[61]"),
            gradient,
            colour_ms: time("live.text[42]", "live.numbox[46]", "live.numbox[47]"),
            length_ms: time("live.text[39]", "live.numbox[43]", "live.numbox[44]"),
            loop_gradient: on("live.text[34]"),
        }
    }

    /// What a MIDI Launcher plays: its file, or nothing when it has none.
    fn midi_file(&mut self, device: &Node) -> MidiFile {
        let mut drops = Vec::new();
        if let Some(list) = device.at("FileDropList") {
            list.find_all("FileRef", &mut drops);
        }
        // Live keeps the file's reference in a reference of its own.
        let Some(file) = drops
            .iter()
            .find_map(|node| FileRef::read(node.child("FileRef").unwrap_or(node)))
        else {
            // One with no file, which some branches hold: it plays nothing.
            return MidiFile::default();
        };
        let name = file.name().unwrap_or("?").to_owned();
        let Some(path) = self.resolver.resolve(&file, self.bases) else {
            self.missing.push(name);
            return MidiFile::default();
        };
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| read_midi_file(&bytes))
        {
            Ok(midi) => midi,
            Err(e) => {
                self.missing.push(format!("{name} ({e})"));
                MidiFile::default()
            }
        }
    }

    fn rack(&mut self, device: &Node, page_switched: bool) -> Rack {
        let selector = device.child("ChainSelector");
        let mapped = selector.and_then(|selector| {
            let key = selector.child("KeyMidi")?;
            if key.value_at("IsNote") != Some("true") {
                return None;
            }
            let low = key.parse_at::<i32>("LowerRangeNote").filter(|&n| n >= 0)?;
            let high = key
                .parse_at::<i32>("UpperRangeNote")
                .filter(|&n| n >= low)?;
            Some(Selector::Page {
                first: selector.parse_at("MidiControllerRange/Min").unwrap_or(0.0),
                last: selector
                    .parse_at("MidiControllerRange/Max")
                    .unwrap_or(127.0),
                pages: (high - low + 1).clamp(1, 128) as u8,
            })
        });
        let selector = mapped.unwrap_or_else(|| {
            if page_switched {
                Selector::PageNumber
            } else {
                Selector::Fixed(selector.and_then(|s| s.parse_at("Manual")).unwrap_or(0.0))
            }
        });
        let branches = device
            .child("Branches")
            .into_iter()
            .flat_map(|branches| branches.children("MidiEffectBranch"))
            .map(|branch| {
                let range = |path: &str, low: f32, high: f32| -> (f32, f32) {
                    let zone = branch.at(path);
                    let min = zone.and_then(|z| z.parse_at("Min")).unwrap_or(low);
                    let max = zone.and_then(|z| z.parse_at("Max")).unwrap_or(high);
                    (min, max)
                };
                let (key_low, key_high) = range("ZoneSettings/KeyRange", 0.0, 127.0);
                let (velocity_low, velocity_high) = range("ZoneSettings/VelocityRange", 1.0, 127.0);
                Branch {
                    keys: (byte(key_low), byte(key_high)),
                    velocities: (byte(velocity_low), byte(velocity_high)),
                    selector: range("BranchSelectorRange", 0.0, 127.0),
                    chain: branch
                        .at("DeviceChain/MidiToMidiDeviceChain/Devices")
                        .map(|devices| self.chain(devices))
                        .unwrap_or_default(),
                }
            })
            .collect();
        Rack { selector, branches }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_of_a_midi_file() {
        // Format 0, 96 ticks a beat: a note at once, its end a beat later
        // and another started then, as running status writes them.
        let track: &[u8] = &[
            0x00, 0x90, 36, 3, //
            0x60, 0x90, 36, 0, //
            0x00, 37, 5, //
            0x30, 0x80, 37, 0, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec();
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
        let file = read_midi_file(&bytes).unwrap();
        let notes: Vec<(f64, u8, u8)> = file
            .notes
            .iter()
            .map(|n| (n.beat, n.note, n.velocity))
            .collect();
        assert_eq!(
            notes,
            [(0.0, 36, 3), (1.0, 36, 0), (1.0, 37, 5), (1.5, 37, 0)]
        );
    }
}
