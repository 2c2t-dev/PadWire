//! UniPacks, the covers of the UniPad community: an `info` file, a
//! `keySound` file, the `sounds/` they play, and the lights of `keyLED/`.
//!
//! The documentation is at <https://unipad.io/docs/unipack>. Where it says
//! too little, this reads packs as UniPad's Android app does
//! (<https://github.com/kimjisub/unipad-android>):
//!
//! - a pad is given as its row from the top, then its column from the
//!   left, both from 1 (UniPad's drivers send `10 * (8 - x) + y + 1` for
//!   the pad `x y` counted from 0);
//! - `mc 1` to `mc 32` are the round buttons all around, clockwise from the
//!   left of the top row: `mc 1`–`8` the top row, `mc 9`–`16` the right side
//!   from the top, then the bottom row and the left side, which only the
//!   Pro has;
//! - the page buttons are the right side, the first page at the top;
//! - a sound's wormhole turns the page as soon as it is pressed.
//!
//! `autoPlay`, the cover played through, is read as UniPad reads it: `on`,
//! `off` and `touch` a pad, `chain` to turn the page, which starts every
//! pad's sounds over, and `delay`; what is not apart by a delay is at once.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Cover, Error, FileSource, Format, decode_text, source::normalise};
use crate::model::{
    AutoAction, AutoEvent, AutoPlay, LedAction, LedColor, LedEvent, LedProgram, LedSequence,
    LedTarget, LoopMode, Page, Project, SampleFile, SoundRef,
};

/// The most pages a UniPack may have.
pub const MAX_CHAINS: usize = 24;

/// Whether these files are a UniPack.
pub fn is_unipack(files: &dyn FileSource) -> bool {
    root(files).is_some()
}

/// Where the pack starts among its files, as a prefix of their paths: an
/// archive often holds the pack in a folder of its own. The shallowest
/// folder with both an `info` and a `keySound` is taken.
fn root(files: &dyn FileSource) -> Option<String> {
    files
        .listing()
        .paths()
        .iter()
        .filter_map(|path| {
            let (dir, name) = match path.rsplit_once('/') {
                Some((dir, name)) => (format!("{dir}/"), name),
                None => (String::new(), path.as_str()),
            };
            (name.eq_ignore_ascii_case("info")).then_some(dir)
        })
        .filter(|dir| files.find(&format!("{dir}keySound")).is_some())
        .min_by_key(|dir| (dir.matches('/').count(), dir.clone()))
}

/// Read a UniPack.
pub fn read(files: Arc<dyn FileSource>) -> Result<Cover, Error> {
    let root = root(&*files).ok_or(Error::NotACover)?;
    let mut warnings = Vec::new();

    let info = parse_info(
        &decode_text(&files.read(&format!("{root}info"))?),
        &mut warnings,
    )?;
    let mut project = Project {
        title: info.title.clone(),
        author: info.producer.clone(),
        size: (info.columns, info.rows),
        pages: vec![Page::default(); info.chains],
        samples: Vec::new(),
        midi_lights: None,
        autoplay: None,
    };

    // Sounds, each file once however many pads play it.
    let mut samples: HashMap<String, usize> = HashMap::new();
    let key_sound = decode_text(&files.read(&format!("{root}keySound"))?);
    let mut missing = Vec::new();
    for line in parse_key_sound(&key_sound, &info, &mut warnings) {
        let wanted = format!("{root}sounds/{}", line.file);
        let Some(listed) = files.find(&wanted) else {
            missing.push(line.file.clone());
            continue;
        };
        let next = project.samples.len();
        let sample = *samples.entry(listed.to_owned()).or_insert(next);
        if sample == next {
            project.samples.push(SampleFile {
                path: listed.to_owned(),
                range: None,
            });
        }
        let pad = project.pages[line.page].pads.entry(line.pad).or_default();
        pad.sounds.push(SoundRef {
            sample,
            loop_mode: line.loop_mode,
            wormhole: line.wormhole,
            gain: 1.0,
        });
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        warnings.push(format!(
            "{} sound file(s) are missing from sounds/: {}",
            missing.len(),
            list_some(&missing)
        ));
    }

    // Lights, in the order of their files' names, which is the order a
    // pad's presses go through them.
    let led_dir = normalise(&format!("{root}keyLED"));
    let mut led_files: Vec<&str> = files
        .listing()
        .paths()
        .iter()
        .filter(|path| {
            let normal = normalise(path);
            normal
                .rsplit_once('/')
                .is_some_and(|(dir, _)| dir == led_dir)
        })
        .map(String::as_str)
        .collect();
    led_files.sort_by_key(|path| path.to_lowercase());
    for path in led_files {
        let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
        let Some((page, pad, loop_mode)) = parse_led_name(name, &info) else {
            warnings.push(format!("keyLED/{name}: not a pad's lights, skipped"));
            continue;
        };
        let text = decode_text(&files.read_listed(path)?);
        let sequence = parse_key_led(&text, &info, loop_mode, name, &mut warnings);
        let pad = project.pages[page].pads.entry(pad).or_default();
        pad.leds.push(LedProgram::Sequence(Arc::new(sequence)));
    }

    if let Some(path) = files.find(&format!("{root}autoPlay")) {
        let text = decode_text(&files.read_listed(path)?);
        let autoplay = parse_auto_play(&text, &info, &mut warnings);
        if !autoplay.events.is_empty() {
            project.autoplay = Some(Arc::new(autoplay));
        }
    }

    Ok(Cover {
        project,
        format: Format::UniPack,
        files,
        warnings,
    })
}

/// A few of many names, for a message.
fn list_some(names: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut list = names[..names.len().min(SHOWN)].join(", ");
    if names.len() > SHOWN {
        list.push_str(", …");
    }
    list
}

/// What `info` says.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub title: String,
    pub producer: Option<String>,
    /// `buttonX`: UniPad's x counts rows.
    pub rows: u8,
    /// `buttonY`.
    pub columns: u8,
    pub chains: usize,
    pub square_buttons: bool,
    pub website: Option<String>,
}

/// The words of a line, apart at any space, the no-break and ideographic
/// ones included, with the byte order mark Notepad leaves at the start.
fn words(line: &str) -> impl Iterator<Item = &str> {
    line.trim_start_matches('\u{feff}').split_whitespace()
}

pub fn parse_info(text: &str, warnings: &mut Vec<String>) -> Result<Info, Error> {
    let mut values: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if line.is_empty() {
            continue;
        }
        match line.split_once('=') {
            Some((key, value)) => {
                values.insert(key.trim(), value.trim());
            }
            None => warnings.push(format!("info: “{line}” is not a key=value line")),
        }
    }
    let number = |key: &str| -> Result<Option<u32>, Error> {
        values
            .get(key)
            .map(|v| v.parse::<u32>())
            .transpose()
            .map_err(|_| Error::Invalid(format!("info: {key} is not a number")))
    };
    let rows = number("buttonX")?.unwrap_or_else(|| {
        warnings.push("info: buttonX is missing, 8 is taken".into());
        8
    });
    let columns = number("buttonY")?.unwrap_or_else(|| {
        warnings.push("info: buttonY is missing, 8 is taken".into());
        8
    });
    if !(1..=64).contains(&rows) || !(1..=64).contains(&columns) {
        return Err(Error::Invalid(format!(
            "info: a grid of {rows}×{columns} is not one PadWire plays"
        )));
    }
    let chains =
        number("chain")?.ok_or_else(|| Error::Invalid("info: chain is missing".into()))? as usize;
    if !(1..=MAX_CHAINS).contains(&chains) {
        return Err(Error::Invalid(format!(
            "info: {chains} chains, where a UniPack has 1 to 24"
        )));
    }
    let title = match values.get("title") {
        Some(title) if !title.is_empty() => title.to_string(),
        _ => {
            warnings.push("info: title is missing".into());
            "Untitled".into()
        }
    };
    let text = |key: &str| {
        values
            .get(key)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
    };
    Ok(Info {
        title,
        producer: text("producerName"),
        rows: rows as u8,
        columns: columns as u8,
        chains,
        square_buttons: values.get("squareButton").is_none_or(|v| *v != "false"),
        website: text("website"),
    })
}

/// A line of `keySound`.
#[derive(Debug, Clone, PartialEq)]
pub struct KeySoundLine {
    pub page: usize,
    pub pad: (u8, u8),
    pub file: String,
    pub loop_mode: LoopMode,
    pub wormhole: Option<usize>,
}

/// The lines of `keySound`: `chain x y file [loop] [wormhole]`.
pub fn parse_key_sound(text: &str, info: &Info, warnings: &mut Vec<String>) -> Vec<KeySoundLine> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let words: Vec<&str> = words(line).collect();
        if words.len() < 4 {
            if words.len() == 3 {
                warnings.push(format!("keySound: “{}” names no file", line.trim()));
            }
            continue;
        }
        let parsed = (|| -> Option<KeySoundLine> {
            let chain: usize = words[0].parse().ok()?;
            let row: u8 = words[1].parse().ok()?;
            let column: u8 = words[2].parse().ok()?;
            let loop_mode = match words.get(4) {
                None => LoopMode::Once,
                Some(word) => match word.parse::<u32>().ok()? {
                    0 => LoopMode::Infinite,
                    1 => LoopMode::Once,
                    n => LoopMode::Times(n),
                },
            };
            // 0 is UniPad's "none".
            let wormhole = match words.get(5) {
                None => None,
                Some(word) => word.parse::<usize>().ok()?.checked_sub(1),
            };
            Some(KeySoundLine {
                page: chain.checked_sub(1)?,
                pad: to_pad(row, column, info)?,
                file: words[3].to_owned(),
                loop_mode,
                wormhole,
            })
        })();
        match parsed {
            Some(parsed) if parsed.page < info.chains => {
                let mut parsed = parsed;
                if parsed.wormhole.is_some_and(|page| page >= info.chains) {
                    warnings.push(format!(
                        "keySound: “{}” goes to a chain there is not",
                        line.trim()
                    ));
                    parsed.wormhole = None;
                }
                lines.push(parsed);
            }
            _ => warnings.push(format!(
                "keySound: “{}” is not a sound of a pad",
                line.trim()
            )),
        }
    }
    lines
}

/// A pad as the model has it, from UniPad's row from the top and column.
fn to_pad(row: u8, column: u8, info: &Info) -> Option<(u8, u8)> {
    if !(1..=info.rows).contains(&row) || !(1..=info.columns).contains(&column) {
        return None;
    }
    Some((column, info.rows + 1 - row))
}

/// What the name of a `keyLED` file says: `chain x y [loop]`, and
/// whatever follows, which only tells apart files of the same pad.
pub fn parse_led_name(name: &str, info: &Info) -> Option<(usize, (u8, u8), LoopMode)> {
    let words: Vec<&str> = words(name).collect();
    if words.len() < 3 {
        return None;
    }
    let chain: usize = words[0].parse().ok()?;
    let page = chain.checked_sub(1).filter(|&page| page < info.chains)?;
    let pad = to_pad(words[1].parse().ok()?, words[2].parse().ok()?, info)?;
    let loop_mode = match words.get(3) {
        None => LoopMode::Once,
        Some(word) => match word.parse::<u32>().ok()? {
            0 => LoopMode::Infinite,
            1 => LoopMode::Once,
            n => LoopMode::Times(n),
        },
    };
    Some((page, pad, loop_mode))
}

/// The lights of a `keyLED` file.
pub fn parse_key_led(
    text: &str,
    info: &Info,
    loop_mode: LoopMode,
    name: &str,
    warnings: &mut Vec<String>,
) -> LedSequence {
    let mut events = Vec::new();
    let mut at_ms: u32 = 0;
    let mut wrong = 0usize;
    for line in text.lines() {
        let words: Vec<&str> = words(line).collect();
        let Some(&command) = words.first() else {
            continue;
        };
        let action = match command {
            "on" | "o" => match target(&words[1..], info) {
                Some(Lit::Led(target, rest)) => match color(rest) {
                    Some(color) => Some(LedAction::Set(target, color)),
                    None => {
                        wrong += 1;
                        None
                    }
                },
                Some(Lit::Ignored) => None,
                None => {
                    wrong += 1;
                    None
                }
            },
            "off" | "f" => match target(&words[1..], info) {
                Some(Lit::Led(target, _)) => Some(LedAction::Set(target, LedColor::Off)),
                Some(Lit::Ignored) => None,
                None => {
                    wrong += 1;
                    None
                }
            },
            "delay" | "d" => {
                match words.get(1).and_then(|w| w.parse::<u32>().ok()) {
                    Some(ms) => at_ms = at_ms.saturating_add(ms),
                    None => wrong += 1,
                }
                None
            }
            "chain" | "c" => match words.get(1).and_then(|w| w.parse::<usize>().ok()) {
                Some(chain) if (1..=info.chains).contains(&chain) => {
                    Some(LedAction::Page(chain - 1))
                }
                _ => {
                    wrong += 1;
                    None
                }
            },
            _ => {
                wrong += 1;
                None
            }
        };
        if let Some(action) = action {
            events.push(LedEvent { at_ms, action });
        }
    }
    if wrong > 0 {
        warnings.push(format!(
            "keyLED/{name}: {wrong} line(s) not understood, skipped"
        ));
    }
    LedSequence {
        events,
        duration_ms: at_ms,
        loop_mode,
    }
}

/// The lines of `autoPlay`.
pub fn parse_auto_play(text: &str, info: &Info, warnings: &mut Vec<String>) -> AutoPlay {
    let mut events = Vec::new();
    let mut at_ms = 0.0f64;
    let mut wrong = 0usize;
    for line in text.lines() {
        let words: Vec<&str> = words(line).collect();
        let Some(&command) = words.first() else {
            continue;
        };
        let pad = || -> Option<(u8, u8)> {
            to_pad(
                words.get(1)?.parse().ok()?,
                words.get(2)?.parse().ok()?,
                info,
            )
        };
        let mut push = |action| events.push(AutoEvent { at_ms, action });
        match command {
            "on" | "o" => match pad() {
                Some((x, y)) => push(AutoAction::Press(x, y)),
                None => wrong += 1,
            },
            "off" | "f" => match pad() {
                Some((x, y)) => push(AutoAction::Release(x, y)),
                None => wrong += 1,
            },
            "touch" | "t" => match pad() {
                Some((x, y)) => {
                    push(AutoAction::Press(x, y));
                    push(AutoAction::Release(x, y));
                }
                None => wrong += 1,
            },
            "chain" | "c" => match words.get(1).and_then(|w| w.parse::<usize>().ok()) {
                Some(chain) if (1..=info.chains).contains(&chain) => push(AutoAction::Page {
                    page: chain - 1,
                    restart_sounds: true,
                }),
                _ => wrong += 1,
            },
            "delay" | "d" => match words.get(1).and_then(|w| w.parse::<f64>().ok()) {
                Some(ms) if ms >= 0.0 => at_ms += ms,
                _ => wrong += 1,
            },
            _ => wrong += 1,
        }
    }
    if wrong > 0 {
        warnings.push(format!("autoPlay: {wrong} line(s) not understood, skipped"));
    }
    AutoPlay { events }
}

/// What an `on` or `off` line lights.
enum Lit<'a> {
    /// A light, and the words after it.
    Led(LedTarget, &'a [&'a str]),
    /// One PadWire has no use for: the logo, or a round button only the
    /// Pro has.
    Ignored,
}

fn target<'a>(words: &'a [&'a str], info: &Info) -> Option<Lit<'a>> {
    match *words.first()? {
        "*" | "mc" => {
            let n: u8 = words.get(1)?.parse().ok()?;
            Some(match n {
                1..=8 => Lit::Led(LedTarget::Top(n), &words[2..]),
                9..=16 => Lit::Led(LedTarget::Side(17 - n), &words[2..]),
                17..=32 => Lit::Ignored,
                _ => return None,
            })
        }
        // The logo, with a placeholder where the column goes, or not.
        "l" => Some(Lit::Ignored),
        row => {
            let (x, y) = to_pad(row.parse().ok()?, words.get(1)?.parse().ok()?, info)?;
            Some(Lit::Led(LedTarget::Grid(x, y), &words[2..]))
        }
    }
}

/// `RRGGBB`, `RRGGBB velocity` or `auto velocity`.
fn color(words: &[&str]) -> Option<LedColor> {
    let velocity = |word: &str| word.parse::<u8>().ok().filter(|v| *v < 128);
    match *words {
        [hex] => Some(LedColor::Rgb {
            rgb: rgb(hex)?,
            palette: None,
        }),
        ["auto" | "a", v] => Some(LedColor::Palette(velocity(v)?)),
        [hex, v] => Some(LedColor::Rgb {
            rgb: rgb(hex)?,
            palette: Some(velocity(v)?),
        }),
        _ => None,
    }
}

fn rgb(hex: &str) -> Option<[u8; 3]> {
    if hex.is_empty() || hex.len() > 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Info {
        parse_info(
            "title=T\nproducerName=P\nbuttonX=8\nbuttonY=8\nchain=2\n",
            &mut Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn info_lines() {
        let mut warnings = Vec::new();
        let info = parse_info(
            "\u{feff}title = Faded\r\nproducerName=Someone\nbuttonX=8\nbuttonY=8\nchain=6\nsquareButton=false\n",
            &mut warnings,
        )
        .unwrap();
        assert_eq!(info.title, "Faded");
        assert_eq!(info.producer.as_deref(), Some("Someone"));
        assert_eq!((info.rows, info.columns, info.chains), (8, 8, 6));
        assert!(!info.square_buttons);
        assert!(warnings.is_empty());
        assert!(parse_info("title=T\nchain=25\n", &mut Vec::new()).is_err());
    }

    #[test]
    fn rows_count_from_the_top() {
        let info = info();
        // The top left pad, then the bottom right one.
        assert_eq!(to_pad(1, 1, &info), Some((1, 8)));
        assert_eq!(to_pad(8, 8, &info), Some((8, 1)));
        assert_eq!(to_pad(9, 1, &info), None);
    }

    #[test]
    fn key_sound_lines() {
        let mut warnings = Vec::new();
        let lines = parse_key_sound(
            "1 1 1 a.wav\n1 1 1 b.wav 0\n2\u{3000}8 8 c.wav 3 1\n1 1 1\n3 1 1 d.wav\n1 1 1 e.wav 1 0\n",
            &info(),
            &mut warnings,
        );
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].loop_mode, LoopMode::Once);
        assert_eq!(lines[1].loop_mode, LoopMode::Infinite);
        assert_eq!(lines[2].page, 1);
        assert_eq!(lines[2].pad, (8, 1));
        assert_eq!(lines[2].loop_mode, LoopMode::Times(3));
        assert_eq!(lines[2].wormhole, Some(0));
        assert_eq!(lines[3].wormhole, None);
        // The line without a file, and the one of a chain there is not.
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn key_led_lines() {
        let mut warnings = Vec::new();
        let text = "o 1 1 ff0000\non 8 8 auto 5\nd 100\no mc 9 00ff00 21\no * 1 a 3\n\
                    f 1 1\no l 0000ff\nd 50\nc 2\nbogus\n";
        let sequence = parse_key_led(text, &info(), LoopMode::Once, "1 1 1", &mut warnings);
        use LedAction::*;
        use LedTarget::*;
        let actions: Vec<(u32, LedAction)> = sequence
            .events
            .iter()
            .map(|e| (e.at_ms, e.action))
            .collect();
        assert_eq!(
            actions,
            vec![
                (
                    0,
                    Set(
                        Grid(1, 8),
                        LedColor::Rgb {
                            rgb: [255, 0, 0],
                            palette: None
                        }
                    )
                ),
                (0, Set(Grid(8, 1), LedColor::Palette(5))),
                (
                    100,
                    Set(
                        Side(8),
                        LedColor::Rgb {
                            rgb: [0, 255, 0],
                            palette: Some(21)
                        }
                    )
                ),
                (100, Set(Top(1), LedColor::Palette(3))),
                (100, Set(Grid(1, 8), LedColor::Off)),
                (150, Page(1)),
            ]
        );
        assert_eq!(sequence.duration_ms, 150);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn auto_play_lines() {
        let mut warnings = Vec::new();
        let text = "c 2\no 1 1\nt 8 8\nd 250\nf 1 1\ndelay 100\nchain 1\nwhat\n";
        let auto = parse_auto_play(text, &info(), &mut warnings);
        let events: Vec<(f64, AutoAction)> =
            auto.events.iter().map(|e| (e.at_ms, e.action)).collect();
        assert_eq!(
            events,
            [
                (
                    0.0,
                    AutoAction::Page {
                        page: 1,
                        restart_sounds: true
                    }
                ),
                (0.0, AutoAction::Press(1, 8)),
                (0.0, AutoAction::Press(8, 1)),
                (0.0, AutoAction::Release(8, 1)),
                (250.0, AutoAction::Release(1, 8)),
                (
                    350.0,
                    AutoAction::Page {
                        page: 0,
                        restart_sounds: true
                    }
                ),
            ]
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn led_file_names() {
        let info = info();
        assert_eq!(
            parse_led_name("1 2 3", &info),
            Some((0, (3, 7), LoopMode::Once))
        );
        assert_eq!(
            parse_led_name("2 1 1 0 a", &info),
            Some((1, (1, 8), LoopMode::Infinite))
        );
        assert_eq!(
            parse_led_name("2 1 1 4", &info),
            Some((1, (1, 8), LoopMode::Times(4)))
        );
        assert_eq!(parse_led_name("3 1 1", &info), None);
        assert_eq!(parse_led_name("readme.txt", &info), None);
    }
}
