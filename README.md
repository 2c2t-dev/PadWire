# PadWire

Launchpad covers, played on Linux without Ableton. PadWire = Launch**Pad** +
Pipe**Wire**.

- Opens **UniPacks**, the covers of the UniPad community, zipped or as a
  folder, and **Ableton covers**: the `.als` set, whose samples are found
  again wherever the cover was unpacked.
- Plays their sounds with little latency, through PipeWire.
- Lights a real Launchpad (the Mini MK3 for now) as the cover says, and
  shows the same lights on screen, where the pads can be played with the
  mouse. The lights of Ableton covers come from their racks of MIDI
  effects (Pitch, Chord, Velocity, Note Length, Arpeggiator), which
  PadWire plays as Live would.
- Plays a cover through by itself, or **practises** it with you: the pads
  to press next light up, and it waits for them. The performance comes
  from a UniPack's `autoPlay`, or from an Ableton cover's tutorial clip or
  clip of the whole song.

## Building

PadWire needs Rust, GTK 4.18 or later, libadwaita 1.7 or later, and the
development files of ALSA and PipeWire.

```sh
cargo run --release
```

A cover can be given on the command line, `padwire cover.zip`, opened from
the window, or dropped on it.

## Without a Launchpad

`examples/fake_launchpad.rs` makes virtual MIDI ports by the names of a
Mini MK3's. PadWire connects to it as to the real one, and it prints what
PadWire lights:

```sh
cargo run --example fake_launchpad
```

`tests/fake_device.rs` does the same in the tests, wherever there is an
ALSA sequencer.

`examples/inspect_cover.rs` prints what PadWire reads of a cover, page by
page, without playing it, and with `--lights PAGE X Y` what a press of a
pad lights, in time:

```sh
cargo run --release --example inspect_cover -- --lights 1 4 3 path/to/cover.als
```

`PADWIRE_APP_ID=dev._2c2t.padwire.Test` runs a second PadWire beside the
one already running, rather than handing it the cover.

## How it is made

| Module | What it does |
| --- | --- |
| `model` | A cover, whatever its format: pages of pads, their sounds and lights |
| `formats` | Covers read into the model: `unipack`, and `als` for Ableton Live Sets |
| `engine::audio` | The mixer, on the audio thread, told what to play through a lock-free ring buffer, and played through cpal |
| `engine::decode` | Every sound decoded into memory at the output's rate before the cover plays |
| `engine::led` | The lights' programs, played on a clock of their own thread |
| `engine::midifx` | Ableton's MIDI effects, played in that thread for the lights of `.als` covers |
| `engine::session` | What a press does, from the Launchpad or the window |
| `device` | The Launchpad found, connected and given back; one profile per model, and the note layouts |
| `ui` | The window, in GTK 4 and libadwaita |

Settings and the library are kept in `~/.config/padwire/config.toml`.

## Roadmap

| Version | What |
| --- | --- |
| 0.1 | UniPacks, the Mini MK3, the grid on screen |
| 0.2 | Ableton `.als` covers: sounds, pages, mappings, sample paths found again |
| **0.3** | Ableton's MIDI effects emulated, for the lights of `.als` covers |
| 0.4 | autoPlay and practice mode (done); other Launchpads, Flatpak |

## Credits

The Launchpad palette comes from [Apollo Studio](https://github.com/mat1jaczyyy/apollo-studio),
© 2019 mat1jaczyyy, under the BSD 3-Clause licence.
