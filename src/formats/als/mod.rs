//! Ableton Live Sets made as Launchpad covers: their sounds, their pages
//! and the pads that play them.
//!
//! What such a set is, as the Faded cover by Hajime0512 shows, in Live 9
//! as resaved by Live 12:
//!
//! - a track of sounds holds a Drum Rack, a branch a pad, by the note it
//!   receives in the Drum Rack layout (36 to 99, the Launchpad's User mode);
//!   Live keeps that note as `128 - note`;
//! - in each branch, an Instrument Rack chooses the sound of each page by
//!   its Chain Selector: each of its branches plays for a range of values,
//!   then a Simpler plays a sample;
//! - a macro of the Drum Rack is mapped to the notes of the round buttons
//!   of the side (100 to 107, on channel 6) over the values 0 to 7: the
//!   button pressed sets the page. The set does not say where the macro
//!   goes; covers map it to the Chain Selector of every pad's rack, so the
//!   macro's value is taken as theirs.
//!
//! The lights come from other tracks, of MIDI effects: see [`lights`].

mod autoplay;
mod lights;
mod paths;
pub mod xml;

pub use paths::ableton_libraries;

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::GzDecoder;

use super::source::Listing;
use super::{Cover, Error, FileSource, Format};
use crate::device::layout::drum_rack;
use crate::model::{LoopMode, Pad, Page, PlaybackMode, Project, SampleFile, SoundRef};
use paths::{FileRef, Resolver};
use xml::Node;

/// A release this long is as good as none: the sound plays to its end.
const ENDLESS_RELEASE_MS: f32 = 5000.0;
/// The most pages read from a mapping.
const MAX_PAGES: usize = 128;

/// What the set's XML is read without: the automation of every parameter,
/// presets, binary aliases, and what only Live's window shows. Kept are the
/// clips, in the session and in the arrangement (whose clips Live keeps in
/// the arrangement's automation), with their envelopes, and
/// the ids parameters are automated by, which envelopes point to.
fn skipped(tag: &str, parent: &str) -> bool {
    if tag == "ArrangerAutomation" {
        // Live 9 keeps the arrangement's clips in a `Sample`, Live 11 and
        // after in a `ClipTimeable`.
        return !matches!(parent, "Sample" | "ClipTimeable");
    }
    matches!(
        tag,
        "LockEnvelope"
            | "ModulationTarget"
            | "ModulationAutomation"
            | "Data"
            | "SourceContext"
            | "LastPresetRef"
            | "FreezeSequencer"
            | "ClipSlotsListWrapper"
            | "WarpMarkers"
            | "AutomationEnvelopes"
            | "Lfo"
            | "AuxLfos.0"
            | "AuxLfos.1"
            | "AuxEnv"
            | "Filter"
            | "Shaper"
            | "ViewSettings"
            | "SimplerSlicing"
            | "Locators"
            | "Scenes"
            | "Transport"
            | "ViewStates"
            // What only Live's window shows.
            | "LomId"
            | "LomIdView"
            | "IsExpanded"
            | "IsFolded"
            | "IsSelected"
            | "IsSoloed"
            | "ColorIndex"
            | "AutoColored"
            | "Annotation"
            | "SessionViewBranchWidth"
            | "IsHighlightedInSessionView"
            | "SoloActivatedInSessionMixer"
            | "DevicesListWrapper"
            | "ParametersListWrapper"
            | "ChainsListWrapper"
            | "ReturnChainsListWrapper"
            | "MidiCCOnOffThresholds"
            | "LastSelectedTimeableIndex"
            | "LastSelectedClipEnvelopeIndex"
            | "LockedScripts"
            | "ShouldShowPresetName"
            | "OverwriteProtectionNumber"
            | "ViewsToRestoreWhenUnfolding"
            | "BranchesSplitterProportion"
            | "ShowBranchesInSessionMixer"
            | "LockId"
            | "LockSeal"
    ) || tag.starts_with("MacroDisplayNames")
        || tag.starts_with("MacroDefaults")
        || tag.starts_with("MacroAnnotations")
        || tag.starts_with("MacroColorIndex")
        || tag.starts_with("ForceDisplayGenericValue")
        || tag.contains("ScrollPos")
        || tag.ends_with("RelativePosition")
}

/// Whether a file is a Live Set.
pub fn is_set(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("als"))
}

/// The sets in a folder and the folders in it, but not Live's backups.
pub fn sets_in(dir: &Path) -> Vec<PathBuf> {
    let mut sets = Vec::new();
    let look = |dir: &Path, sets: &mut Vec<PathBuf>, deeper: &mut Vec<PathBuf>| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !entry.file_name().eq_ignore_ascii_case("Backup") {
                    deeper.push(path);
                }
            } else if is_set(&path) {
                sets.push(path);
            }
        }
    };
    let mut deeper = Vec::new();
    look(dir, &mut sets, &mut deeper);
    if sets.is_empty() {
        for dir in deeper {
            look(&dir, &mut sets, &mut Vec::new());
        }
    }
    sets.sort();
    sets
}

/// Read a Live Set.
pub fn read(path: &Path, options: &super::Options) -> Result<Cover, Error> {
    let file = File::open(path)?;
    let document = xml::parse(
        BufReader::new(GzDecoder::new(BufReader::new(file))),
        skipped,
    )
    .map_err(|e| Error::Invalid(format!("not a Live Set PadWire reads: {e}")))?;
    let set = document
        .child("LiveSet")
        .ok_or_else(|| Error::Invalid("not a Live Set: there is no LiveSet in it".into()))?;
    let title = path.file_stem().map_or_else(
        || "Untitled".into(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    read_set(set, path, title, options)
}

fn read_set(
    set: &Node,
    path: &Path,
    title: String,
    options: &super::Options,
) -> Result<Cover, Error> {
    let mut warnings = Vec::new();
    let set_dir = path.parent().unwrap_or(Path::new(".")).to_owned();
    // A project's folder holds `Ableton Project Info`; covers often keep
    // their samples beside it rather than in it.
    let project_dir = set_dir
        .ancestors()
        .take(3)
        .find(|dir| dir.join("Ableton Project Info").is_dir())
        .map(Path::to_owned);
    let search_root = match &project_dir {
        Some(project) => project.parent().unwrap_or(project).to_owned(),
        None => set_dir.clone(),
    };
    let mut bases = vec![set_dir.clone()];
    bases.extend(project_dir);
    let resolver = Resolver::new(&search_root);
    // Ableton's library, looked through only if a sample is not with the
    // cover.
    let library: OnceCell<Resolver> = OnceCell::new();
    let mut from_library: Vec<String> = Vec::new();

    let Some(sounds) = find_sounds(set, &mut warnings) else {
        return Err(Error::Invalid(
            "this set has no Drum Rack, which a cover plays its sounds from".into(),
        ));
    };
    let track = sounds.track;
    let mapping = sounds.mapping;
    let track_gain = track
        .parse_at::<f32>("DeviceChain/Mixer/Volume/Manual")
        .unwrap_or(1.0);

    let mut files = Files::default();
    let mut pages: Vec<Page> = vec![Page::default(); sounds.pages.len()];
    let mut missing: Vec<String> = Vec::new();
    let mut replaced: Vec<String> = Vec::new();
    let mut off_grid = 0;
    let mut looped = 0;
    let mut sliced = 0;
    for (page, drums) in sounds.pages.iter().enumerate() {
        for &(drum, value, rack_gain) in drums {
            for branch in drum
                .at("Branches")
                .into_iter()
                .flat_map(|b| b.children("DrumBranch"))
            {
                let Some(received) = branch.parse_at::<u8>("BranchInfo/ReceivingNote") else {
                    continue;
                };
                let Some((x, y)) = 128u8.checked_sub(received).and_then(drum_rack::pad) else {
                    // Counted once, not on every page.
                    off_grid += usize::from(page == 0);
                    continue;
                };
                let choke = branch
                    .parse_at::<u8>("BranchInfo/ChokeGroup")
                    .filter(|&group| group > 0);
                let sending = branch
                    .parse_at::<i32>("BranchInfo/SendingNote")
                    .unwrap_or(60);
                let gain = track_gain * rack_gain * mixer_gain(branch);
                let Some(devices) = branch.at("DeviceChain/MidiToAudioDeviceChain/Devices") else {
                    continue;
                };
                let Some((sampler, gain)) = find_sampler(devices, value, gain) else {
                    continue;
                };
                let Some(sound) = Sampler::read(sampler, sending) else {
                    continue;
                };
                let found = resolver.resolve(&sound.file, &bases).or_else(|| {
                    let found = library
                        .get_or_init(|| Resolver::libraries(&options.ableton_libraries))
                        .resolve(&sound.file, &[])?;
                    from_library.push(found.file_name()?.to_string_lossy().into_owned());
                    Some(found)
                });
                // Not there: a sound of the same kind of the cover's own, as a
                // kick for a kick of Ableton's library it was made with.
                let name = sound.file.name().unwrap_or("?").to_owned();
                let found = found.or_else(|| {
                    let alike = resolver.alike(&name)?;
                    let shown = alike
                        .strip_prefix(&search_root)
                        .unwrap_or(&alike)
                        .display()
                        .to_string();
                    replaced.push(format!("{name} by {shown}"));
                    Some(alike)
                });
                let Some(found) = found else {
                    missing.push(name);
                    continue;
                };
                looped += usize::from(sound.looped);
                sliced += usize::from(sound.sliced);
                let sample = files.add(SampleFile {
                    path: found.to_string_lossy().into_owned(),
                    range: sound.range,
                });
                pages[page].pads.insert(
                    (x, y),
                    Pad {
                        sounds: vec![SoundRef {
                            sample,
                            loop_mode: if sound.looped {
                                LoopMode::Infinite
                            } else {
                                LoopMode::Once
                            },
                            wormhole: None,
                            gain: gain * sound.gain,
                        }],
                        leds: Vec::new(),
                        choke_group: choke,
                        playback: sound.playback,
                    },
                );
            }
        }
    }

    // Pages the mapping reaches but nothing plays on, at the end.
    while pages.len() > 1 && pages.last().is_some_and(|page| page.pads.is_empty()) {
        pages.pop();
    }
    if !replaced.is_empty() {
        replaced.sort();
        replaced.dedup();
        warnings.push(format!(
            "{} sample(s) not found, replaced by sounds of the same kind of the cover: {}",
            replaced.len(),
            replaced.join(", ")
        ));
    }
    if !from_library.is_empty() {
        from_library.sort();
        from_library.dedup();
        warnings.push(format!(
            "{} sample(s) taken from Ableton's library rather than the cover: {}",
            from_library.len(),
            from_library.join(", ")
        ));
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        let shown = missing
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "{} sample(s) not found beside the set: {shown}{}",
            missing.len(),
            if missing.len() > 5 { ", …" } else { "" }
        ));
    }
    if off_grid > 0 {
        warnings.push(format!(
            "{off_grid} pad(s) of the Drum Rack are not on the grid, and are not played"
        ));
    }
    if looped > 0 {
        warnings.push(format!(
            "{looped} sample(s) loop in a part of themselves; they loop whole"
        ));
    }
    if sliced > 0 {
        warnings.push(format!("{sliced} sample(s) are sliced; they play whole"));
    }
    if mapping.is_none() && sounds.pages.len() == 1 {
        warnings.push("no page buttons are mapped: the set plays as one page".into());
    }

    let source = Arc::new(files.source(path));
    let page_notes = mapping.and_then(|mapping| u8::try_from(mapping.notes.0).ok());
    let autoplay = autoplay::read(
        set,
        &autoplay::Context {
            sounds: track,
            mapping: mapping.as_ref(),
            page_target: sounds.page_target.as_deref(),
            page_notes: page_notes.unwrap_or(100),
            pages: pages.len(),
            tempo: set
                .parse_at::<f32>("MasterTrack/DeviceChain/Mixer/Tempo/Manual")
                .filter(|t| *t > 0.0)
                .unwrap_or(120.0),
        },
    )
    .filter(|autoplay| !autoplay.events.is_empty())
    .map(Arc::new);
    let midi_lights =
        lights::read(set, track, page_notes, &resolver, &bases, &mut warnings).map(Arc::new);
    let project = Project {
        title,
        author: None,
        size: (8, 8),
        pages,
        samples: files.samples,
        midi_lights,
        autoplay,
    };
    Ok(Cover {
        project,
        format: Format::Ableton,
        files: source,
        warnings,
    })
}

/// A branch of a rack of Drum Racks: the selector values it plays for,
/// its gain, and its Drum Racks.
struct PageBranch<'a> {
    range: (f32, f32),
    gain: f32,
    drums: Vec<&'a Node>,
}

/// Where a set's sounds are, page by page.
struct Sounds<'a> {
    track: &'a Node,
    /// The notes of the page buttons, when they are mapped to notes.
    mapping: Option<PageMapping>,
    /// The id Live automates the control mapped to them by.
    page_target: Option<String>,
    /// For each page, the Drum Racks that play, with the value their racks'
    /// Chain Selectors stand at (`None`: as each stands) and the gain of the
    /// branch they are in.
    pages: Vec<Vec<(&'a Node, Option<u8>, f32)>>,
}

/// The track of sounds. Two ways of making pages are known:
///
/// - one Drum Rack whose pads each choose a sound a page, with a control
///   mapped to the page buttons' notes, as the Faded cover does;
/// - a rack of Drum Racks, a branch a page, its Chain Selector set to the
///   page's number by a page switching device, as Kaskobi's covers do.
fn find_sounds<'a>(set: &'a Node, warnings: &mut Vec<String>) -> Option<Sounds<'a>> {
    let tracks: Vec<&Node> = set.child("Tracks")?.children("MidiTrack").collect();
    // A rack of Drum Racks first: its Drum Racks are pages, not tracks.
    for &track in &tracks {
        let Some(devices) = track.at("DeviceChain/DeviceChain/Devices") else {
            continue;
        };
        let mut racks = Vec::new();
        devices.find_all("InstrumentGroupDevice", &mut racks);
        for rack in racks {
            let branches: Vec<PageBranch> = rack
                .at("Branches")
                .into_iter()
                .flat_map(|b| b.children("InstrumentBranch"))
                .map(|branch| {
                    let mut drums = Vec::new();
                    if let Some(chain) = branch.at("DeviceChain/MidiToAudioDeviceChain/Devices") {
                        chain.find_all("DrumGroupDevice", &mut drums);
                    }
                    let range = (
                        branch
                            .parse_at::<f32>("BranchSelectorRange/Min")
                            .unwrap_or(0.0),
                        branch
                            .parse_at::<f32>("BranchSelectorRange/Max")
                            .unwrap_or(127.0),
                    );
                    PageBranch {
                        range,
                        gain: mixer_gain(branch),
                        drums,
                    }
                })
                .collect();
            if branches.iter().filter(|b| !b.drums.is_empty()).count() < 2 {
                continue;
            }
            let page_target = page_target(rack);
            let mapping = page_mapping(rack);
            let count = match &mapping {
                Some(mapping) => mapping.pages(),
                None => branches
                    .iter()
                    .filter(|b| !b.drums.is_empty())
                    .map(|b| b.range.0 as usize + 1)
                    .max()
                    .unwrap_or(1)
                    .min(MAX_PAGES),
            };
            let pages = (0..count)
                .map(|page| {
                    let value =
                        mapping.map_or(page as f32, |mapping| f32::from(mapping.value(page)));
                    branches
                        .iter()
                        .filter(|b| (b.range.0..=b.range.1).contains(&value))
                        .flat_map(|b| b.drums.iter().map(move |drum| (*drum, None, b.gain)))
                        .collect()
                })
                .collect();
            return Some(Sounds {
                track,
                mapping,
                page_target,
                pages,
            });
        }
    }
    let mut drums: Vec<(&Node, &Node)> = Vec::new();
    for &track in &tracks {
        let mut found = Vec::new();
        if let Some(devices) = track.at("DeviceChain/DeviceChain/Devices") {
            devices.find_all("DrumGroupDevice", &mut found);
        }
        drums.extend(found.into_iter().map(|drum| (track, drum)));
    }
    let &(track, drum) = drums.first()?;
    if drums.len() > 1 {
        warnings.push(format!(
            "{} Drum Racks: only the one of the track “{}” is played",
            drums.len(),
            track_name(track)
        ));
    }
    let mapping = page_mapping(drum);
    let page_target = page_target(drum);
    let pages = match &mapping {
        Some(mapping) => (0..mapping.pages())
            .map(|page| vec![(drum, Some(mapping.value(page)), 1.0)])
            .collect(),
        None => vec![vec![(drum, None, 1.0)]],
    };
    Some(Sounds {
        track,
        mapping,
        page_target,
        pages,
    })
}

fn track_name(track: &Node) -> &str {
    track
        .value_at("Name/EffectiveName")
        .or(track.value_at("Name/UserName"))
        .unwrap_or("?")
}

/// The volume of a rack's branch, which Live keeps as a gain.
fn mixer_gain(branch: &Node) -> f32 {
    branch
        .parse_at::<f32>("MixerDevice/Volume/Manual")
        .unwrap_or(1.0)
}

/// How the page buttons set the page: a control of the Drum Rack mapped to
/// a range of notes, and the values it takes over them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PageMapping {
    notes: (i32, i32),
    values: (f32, f32),
}

impl PageMapping {
    fn pages(&self) -> usize {
        ((self.notes.1 - self.notes.0 + 1).max(1) as usize).min(MAX_PAGES)
    }

    /// The value the control takes on the page's note.
    fn value(&self, page: usize) -> u8 {
        let span = (self.notes.1 - self.notes.0).max(1) as f32;
        let value = self.values.0 + (self.values.1 - self.values.0) * page as f32 / span;
        value.round().clamp(0.0, 127.0) as u8
    }
}

/// A macro or the Chain Selector of the Drum Rack, mapped to notes.
fn page_mapping(drum: &Node) -> Option<PageMapping> {
    page_control(drum).map(|(mapping, _)| mapping)
}

/// The id a rack's control mapped to the page buttons is automated by.
fn page_target(rack: &Node) -> Option<String> {
    let (_, control) = page_control(rack)?;
    control
        .at("AutomationTarget")?
        .attr("Id")
        .map(str::to_owned)
}

/// A macro or the Chain Selector of a rack mapped to notes, and the values
/// it takes over them.
fn page_control(drum: &Node) -> Option<(PageMapping, &Node)> {
    let controls = (0..8)
        .map(|i| format!("MacroControls.{i}"))
        .chain(["ChainSelector".to_owned()]);
    controls
        .filter_map(|control| drum.child(&control))
        .find_map(|control| {
            let key = control.child("KeyMidi")?;
            if key.value_at("IsNote") != Some("true") {
                return None;
            }
            let low = key.parse_at::<i32>("LowerRangeNote").filter(|&n| n >= 0)?;
            let high = key
                .parse_at::<i32>("UpperRangeNote")
                .filter(|&n| n >= low)?;
            let min = control
                .parse_at::<f32>("MidiControllerRange/Min")
                .unwrap_or(0.0);
            let max = control
                .parse_at::<f32>("MidiControllerRange/Max")
                .unwrap_or(127.0);
            Some((
                PageMapping {
                    notes: (low, high),
                    values: (min, max),
                },
                control,
            ))
        })
}

/// The sampler that plays for a value of the page control, under racks
/// whose branches it chooses between, with the gain of the branches on
/// the way. With no mapping, each rack plays as its own selector stands.
fn find_sampler(devices: &Node, value: Option<u8>, gain: f32) -> Option<(&Node, f32)> {
    for device in &devices.children {
        match &*device.tag {
            "OriginalSimpler" | "MultiSampler" => return Some((device, gain)),
            "InstrumentGroupDevice" => {
                let selector = value
                    .map(f32::from)
                    .or_else(|| device.parse_at::<f32>("ChainSelector/Manual"))
                    .unwrap_or(0.0);
                let branches = device.at("Branches")?;
                for branch in branches.children("InstrumentBranch") {
                    let min = branch
                        .parse_at::<f32>("BranchSelectorRange/Min")
                        .unwrap_or(0.0);
                    let max = branch
                        .parse_at::<f32>("BranchSelectorRange/Max")
                        .unwrap_or(127.0);
                    if !(min..=max).contains(&selector) {
                        continue;
                    }
                    let inner = branch.at("DeviceChain/MidiToAudioDeviceChain/Devices")?;
                    if let Some(found) = find_sampler(inner, value, gain * mixer_gain(branch)) {
                        return Some(found);
                    }
                }
                return None;
            }
            // MIDI effects before the sampler, and anything else, change
            // nothing PadWire plays.
            _ => {}
        }
    }
    None
}

/// What a Simpler or a Sampler plays.
struct Sampler {
    file: FileRef,
    range: Option<(u64, u64)>,
    gain: f32,
    playback: PlaybackMode,
    looped: bool,
    sliced: bool,
}

impl Sampler {
    fn read(device: &Node, note: i32) -> Option<Sampler> {
        let parts = device.at("Player/MultiSampleMap/SampleParts")?;
        // The part the note plays, or the first.
        let part = parts
            .children("MultiSamplePart")
            .find(|part| {
                let low = part.parse_at::<i32>("KeyRange/Min").unwrap_or(0);
                let high = part.parse_at::<i32>("KeyRange/Max").unwrap_or(127);
                (low..=high).contains(&note)
            })
            .or_else(|| parts.child("MultiSamplePart"))?;
        let file = FileRef::read(part.at("SampleRef/FileRef")?)?;

        let start = part.parse_at::<u64>("SampleStart").unwrap_or(0);
        let end = part.parse_at::<u64>("SampleEnd");
        let length = part.parse_at::<u64>("SampleRef/DefaultDuration");
        let whole = start == 0
            && match (end, length) {
                (Some(end), Some(length)) => end + 1 >= length,
                _ => true,
            };
        let range = (!whole).then(|| (start, end.unwrap_or(u64::MAX)));

        // Decibels for the device, a gain for the part.
        let decibels = device
            .parse_at::<f32>("VolumeAndPan/Volume/Manual")
            .unwrap_or(0.0);
        let gain = 10f32.powf(decibels / 20.0) * part.parse_at::<f32>("Volume").unwrap_or(1.0);

        // Simpler's modes: 0 classic, 1 one-shot, 2 slicing.
        let mode = device.parse_at::<u8>("Globals/PlaybackMode").unwrap_or(0);
        let release = device
            .parse_at::<f32>("VolumeAndPan/Envelope/ReleaseTime/Manual")
            .unwrap_or(0.0);
        let playback = if mode == 0 && release < ENDLESS_RELEASE_MS {
            PlaybackMode::Gate {
                release_ms: release.max(0.0) as u32,
            }
        } else {
            PlaybackMode::OneShot
        };
        let looped = mode == 0 && part.parse_at::<u8>("SustainLoop/Mode").unwrap_or(0) != 0;
        Some(Sampler {
            file,
            range,
            gain,
            playback,
            looped,
            sliced: mode == 2,
        })
    }
}

/// The samples found, each once.
#[derive(Default)]
struct Files {
    samples: Vec<SampleFile>,
    index: HashMap<SampleFile, usize>,
}

impl Files {
    fn add(&mut self, sample: SampleFile) -> usize {
        let next = self.samples.len();
        *self.index.entry(sample.clone()).or_insert_with(|| {
            self.samples.push(sample);
            next
        })
    }

    fn source(&self, set: &Path) -> SetFiles {
        let mut paths: Vec<String> = self
            .samples
            .iter()
            .map(|sample| sample.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        SetFiles {
            set: set.to_owned(),
            listing: Listing::new(paths),
        }
    }
}

/// The samples of a set, wherever they were found, by their full paths.
struct SetFiles {
    set: PathBuf,
    listing: Listing,
}

impl FileSource for SetFiles {
    fn listing(&self) -> &Listing {
        &self.listing
    }

    fn read_listed(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn origin(&self) -> &Path {
        &self.set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_values_over_the_side_buttons() {
        let mapping = PageMapping {
            notes: (100, 107),
            values: (0.0, 7.0),
        };
        assert_eq!(mapping.pages(), 8);
        assert_eq!(
            (0..8).map(|page| mapping.value(page)).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
        let spread = PageMapping {
            notes: (100, 103),
            values: (0.0, 127.0),
        };
        assert_eq!(
            (0..4).map(|page| spread.value(page)).collect::<Vec<_>>(),
            [0, 42, 85, 127]
        );
    }
}
