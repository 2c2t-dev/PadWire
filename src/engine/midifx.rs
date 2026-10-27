//! Ableton's MIDI effects, played: the notes of the pads go in, the notes
//! that light the Launchpad come out, now or later.
//!
//! Notes go through a chain one device after the other. A rack sends each
//! to its branches, and what comes out of a branch goes on through the rest
//! of the chain the rack is in; where a device holds notes back, a Note
//! Length or an Arpeggiator, what it lets out later is queued with the rest
//! of the way it has to go.
//!
//! As Live does, a rack sends a note's end to the branches its start went
//! to, whatever the page has become meanwhile.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use crate::device::layout::drum_rack;
use crate::model::midifx::{
    Arpeggiator, Chain, Device, MidiFile, MidiLights, Resonator, Selector, VelocityMap,
};

/// A note's start, or its end at velocity 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub note: u8,
    pub velocity: u8,
    /// Which layer it lights in, as Depths Selector sets it: a light shows
    /// the colour of its highest layer lit.
    pub layer: i8,
}

impl Note {
    fn is_on(self) -> bool {
        self.velocity > 0
    }
}

/// The devices of every chain, flat, each by its place: what each one
/// holds is kept by the same place.
struct Graph {
    devices: Vec<Node>,
    chains: Vec<Vec<usize>>,
    /// The chains of the tracks, which notes come in through.
    roots: Vec<usize>,
    /// How long a beat lasts, for the MIDI files.
    beat_ms: f64,
}

enum Node {
    Pitch {
        shift: i32,
        lowest: u8,
        range: u8,
    },
    Chord {
        shifts: Vec<(i32, f32)>,
    },
    Velocity(VelocityMap),
    NoteLength {
        ms: f64,
        on_release: bool,
    },
    Arpeggiator(Arpeggiator),
    Rack {
        selector: Selector,
        branches: Vec<(crate::model::midifx::Branch, usize)>,
    },
    MidiFile(Arc<MidiFile>),
    Resonator(Arc<Resonator>),
    Layer(i8),
}

impl Graph {
    fn new(lights: &MidiLights) -> Self {
        let mut graph = Graph {
            devices: Vec::new(),
            chains: Vec::new(),
            roots: Vec::new(),
            beat_ms: 60_000.0 / f64::from(lights.tempo.max(1.0)),
        };
        for chain in &lights.chains {
            let root = graph.add_chain(chain);
            graph.roots.push(root);
        }
        graph
    }

    fn add_chain(&mut self, chain: &Chain) -> usize {
        let id = self.chains.len();
        self.chains.push(Vec::new());
        let mut devices = Vec::with_capacity(chain.len());
        for device in chain {
            let node = match device {
                Device::Pitch {
                    shift,
                    lowest,
                    range,
                } => Node::Pitch {
                    shift: *shift,
                    lowest: *lowest,
                    range: *range,
                },
                Device::Chord { shifts } => Node::Chord {
                    shifts: shifts.clone(),
                },
                Device::Velocity(map) => Node::Velocity(*map),
                Device::NoteLength { ms, on_release } => Node::NoteLength {
                    ms: f64::from(*ms),
                    on_release: *on_release,
                },
                Device::Arpeggiator(arp) => Node::Arpeggiator(*arp),
                Device::MidiFile(file) => Node::MidiFile(Arc::clone(file)),
                Device::Resonator(resonator) => Node::Resonator(Arc::clone(resonator)),
                Device::Layer(layer) => Node::Layer(*layer),
                Device::Rack(rack) => {
                    let branches = rack
                        .branches
                        .iter()
                        .map(|branch| {
                            let chain = self.add_chain(&branch.chain);
                            let mut zones = branch.clone();
                            zones.chain = Vec::new();
                            (zones, chain)
                        })
                        .collect();
                    Node::Rack {
                        selector: rack.selector,
                        branches,
                    }
                }
            };
            devices.push(self.devices.len());
            self.devices.push(node);
        }
        self.chains[id] = devices;
        id
    }
}

/// Where notes are in a chain: they go on from `index`.
#[derive(Debug, Clone, Copy)]
struct Frame {
    chain: usize,
    index: usize,
}

/// The way still to go: the frame on top first, then the ones under it.
type Stack = Vec<Frame>;

/// What a device holds between notes.
#[derive(Default)]
enum State {
    #[default]
    None,
    /// The branches each note's start went to, by its note, oldest first.
    Rack(HashMap<u8, Vec<Vec<usize>>>),
    /// The velocities of the notes started, for a Note Length on release.
    Started(HashMap<u8, u8>),
    Arpeggiator(ArpState),
    /// A MIDI file playing: which time it is played, and the notes of it
    /// sounding, to end if it starts again.
    File {
        generation: u64,
        sounding: Vec<u8>,
    },
}

#[derive(Default)]
struct ArpState {
    /// The notes held down.
    held: Vec<u8>,
    /// The notes it plays, sorted up, with their velocities.
    notes: Vec<(u8, u8)>,
    /// How many notes it has played since it started.
    step: usize,
    /// When it started.
    start: f64,
    running: bool,
    /// Counts the starts, so that the steps of one before are dropped.
    generation: u64,
}

/// Something due later.
struct Due {
    at: f64,
    order: u64,
    what: Pending,
}

enum Pending {
    /// Notes to send on their way.
    Notes(Vec<Note>, Stack),
    /// An arpeggiator's next note.
    Step {
        device: usize,
        generation: u64,
        stack: Stack,
    },
    /// The next notes of a MIDI file, from `index`, played from `start`.
    FileStep {
        device: usize,
        generation: u64,
        index: usize,
        start: f64,
        stack: Stack,
    },
}

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Due {}

impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Due {
    /// The soonest first in a max-heap, and of two at the same time the one
    /// queued first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .at
            .total_cmp(&self.at)
            .then(other.order.cmp(&self.order))
    }
}

/// The MIDI effects of a cover, playing.
pub struct MidiFx {
    graph: Arc<Graph>,
    states: Vec<State>,
    queue: BinaryHeap<Due>,
    queued: u64,
    page: usize,
}

/// Arpeggiators faster than this are taken as this fast, against a loop.
const SHORTEST_STEP_MS: f64 = 1.0;

impl MidiFx {
    pub fn new(lights: &MidiLights) -> Self {
        let graph = Graph::new(lights);
        let states = graph.devices.iter().map(|_| State::None).collect();
        MidiFx {
            graph: Arc::new(graph),
            states,
            queue: BinaryHeap::new(),
            queued: 0,
            page: 0,
        }
    }

    /// Forget every note and everything due.
    pub fn reset(&mut self) {
        self.states
            .iter_mut()
            .for_each(|state| *state = State::None);
        self.queue.clear();
    }

    pub fn set_page(&mut self, page: usize) {
        self.page = page;
    }

    /// A note from the pads at `now`, in milliseconds; what comes out at
    /// once goes to `out`.
    pub fn input(&mut self, note: Note, now: f64, out: &mut Vec<Note>) {
        for root in self.graph.roots.clone() {
            self.run(
                vec![Frame {
                    chain: root,
                    index: 0,
                }],
                vec![note],
                now,
                out,
            );
        }
    }

    /// When something is next due.
    pub fn next_due(&self) -> Option<f64> {
        self.queue.peek().map(|due| due.at)
    }

    /// Play what is due by `now`.
    pub fn advance(&mut self, now: f64, out: &mut Vec<Note>) {
        while self.queue.peek().is_some_and(|due| due.at <= now) {
            let due = self.queue.pop().expect("peeked");
            match due.what {
                Pending::Notes(notes, stack) => self.run(stack, notes, due.at, out),
                Pending::Step {
                    device,
                    generation,
                    stack,
                } => {
                    let notes = self.step(device, generation, due.at, &stack);
                    if !notes.is_empty() {
                        self.run(stack, notes, due.at, out);
                    }
                }
                Pending::FileStep {
                    device,
                    generation,
                    index,
                    start,
                    stack,
                } => {
                    let notes = self.file_step(device, generation, index, start, &stack);
                    if !notes.is_empty() {
                        self.run(stack, notes, due.at, out);
                    }
                }
            }
        }
    }

    fn later(&mut self, at: f64, what: Pending) {
        self.queued += 1;
        self.queue.push(Due {
            at,
            order: self.queued,
            what,
        });
    }

    /// Send notes on their way at `now`, through what is on the stack.
    fn run(&mut self, mut stack: Stack, mut notes: Vec<Note>, now: f64, out: &mut Vec<Note>) {
        let graph = Arc::clone(&self.graph);
        while let Some(frame) = stack.pop() {
            let chain = &graph.chains[frame.chain];
            for (index, &id) in chain.iter().enumerate().skip(frame.index) {
                // The way on from this device.
                let onward = |stack: &Stack| {
                    let mut onward = stack.clone();
                    onward.push(Frame {
                        chain: frame.chain,
                        index: index + 1,
                    });
                    onward
                };
                match &graph.devices[id] {
                    Node::Pitch {
                        shift,
                        lowest,
                        range,
                    } => {
                        let span = u16::from(*lowest)..=u16::from(*lowest) + u16::from(*range);
                        notes.retain_mut(|note| {
                            if !span.contains(&u16::from(note.note)) {
                                return true;
                            }
                            match u8::try_from(i32::from(note.note) + shift) {
                                Ok(moved) if moved < 128 => {
                                    note.note = moved;
                                    true
                                }
                                _ => false,
                            }
                        });
                    }
                    Node::Chord { shifts } => {
                        let mut chord = Vec::with_capacity(notes.len() * (shifts.len() + 1));
                        for note in &notes {
                            chord.push(*note);
                            for &(shift, part) in shifts {
                                let Ok(pitch) = u8::try_from(i32::from(note.note) + shift) else {
                                    continue;
                                };
                                if pitch > 127 || chord.iter().any(|n: &Note| n.note == pitch) {
                                    continue;
                                }
                                let velocity = if note.is_on() {
                                    (f32::from(note.velocity) * part).round().clamp(1.0, 127.0)
                                        as u8
                                } else {
                                    0
                                };
                                chord.push(Note {
                                    note: pitch,
                                    velocity,
                                    layer: note.layer,
                                });
                            }
                        }
                        notes = chord;
                    }
                    Node::Velocity(map) => {
                        notes.retain_mut(|note| {
                            if !note.is_on() {
                                return true;
                            }
                            match map.map(note.velocity) {
                                Some(velocity) => {
                                    note.velocity = velocity;
                                    true
                                }
                                None => false,
                            }
                        });
                    }
                    Node::NoteLength { ms, on_release } => {
                        if *on_release {
                            let State::Started(started) =
                                self.state(id, || State::Started(HashMap::new()))
                            else {
                                unreachable!()
                            };
                            let mut starts = Vec::new();
                            for note in &notes {
                                if note.is_on() {
                                    started.insert(note.note, note.velocity);
                                } else if let Some(velocity) = started.remove(&note.note) {
                                    starts.push(Note {
                                        note: note.note,
                                        velocity,
                                        layer: note.layer,
                                    });
                                }
                            }
                            notes = starts;
                        } else {
                            notes.retain(|note| note.is_on());
                        }
                        let ends: Vec<Note> = notes
                            .iter()
                            .map(|note| Note {
                                note: note.note,
                                velocity: 0,
                                layer: note.layer,
                            })
                            .collect();
                        if !ends.is_empty() {
                            let onward = onward(&stack);
                            self.later(now + ms, Pending::Notes(ends, onward));
                        }
                    }
                    Node::Arpeggiator(arp) => {
                        let onward = onward(&stack);
                        notes = self.arpeggiate(id, *arp, &notes, now, &onward);
                    }
                    Node::Layer(layer) => {
                        for note in &mut notes {
                            note.layer = *layer;
                        }
                    }
                    Node::Resonator(resonator) => {
                        let onward = onward(&stack);
                        notes = self.resonate(resonator, &notes, now, &onward);
                    }
                    Node::MidiFile(_) => {
                        // A note starts the file over; nothing else gets
                        // through.
                        notes = if notes.iter().any(|note| note.is_on()) {
                            let onward = onward(&stack);
                            self.start_file(id, now, &onward)
                        } else {
                            Vec::new()
                        };
                    }
                    Node::Rack { selector, branches } => {
                        let value = selector.value(self.page);
                        let State::Rack(sent) = self.state(id, || State::Rack(HashMap::new()))
                        else {
                            unreachable!()
                        };
                        // The notes for each branch.
                        let mut routed: Vec<Vec<Note>> = vec![Vec::new(); branches.len()];
                        for note in &notes {
                            let to: Vec<usize> = if note.is_on() {
                                let to: Vec<usize> = (0..branches.len())
                                    .filter(|&b| {
                                        branches[b].0.takes(note.note, note.velocity, value)
                                    })
                                    .collect();
                                sent.entry(note.note).or_default().push(to.clone());
                                to
                            } else {
                                match sent.get_mut(&note.note).filter(|starts| !starts.is_empty()) {
                                    Some(starts) => starts.remove(0),
                                    None => (0..branches.len())
                                        .filter(|&b| branches[b].0.takes(note.note, 0, value))
                                        .collect(),
                                }
                            };
                            for b in to {
                                routed[b].push(*note);
                            }
                        }
                        let onward = onward(&stack);
                        for (b, notes) in routed.into_iter().enumerate() {
                            if notes.is_empty() {
                                continue;
                            }
                            let mut branch = onward.clone();
                            branch.push(Frame {
                                chain: branches[b].1,
                                index: 0,
                            });
                            self.run(branch, notes, now, out);
                        }
                        // The branches took the notes the rest of the way.
                        return;
                    }
                }
                if notes.is_empty() {
                    return;
                }
            }
        }
        out.extend(notes);
    }

    /// A MIDI file played from its start: the notes of the time before end,
    /// then its first notes are played.
    fn start_file(&mut self, id: usize, now: f64, onward: &Stack) -> Vec<Note> {
        let State::File {
            generation,
            sounding,
        } = self.state(id, || State::File {
            generation: 0,
            sounding: Vec::new(),
        })
        else {
            unreachable!()
        };
        *generation += 1;
        let generation = *generation;
        let mut notes: Vec<Note> = sounding
            .drain(..)
            .map(|note| Note {
                note,
                velocity: 0,
                layer: 0,
            })
            .collect();
        let first = match &self.graph.devices[id] {
            Node::MidiFile(file) => file.notes.first().map(|note| note.beat),
            _ => None,
        };
        match first {
            Some(beat) if beat > 0.0 => self.later(
                now + beat * self.graph.beat_ms,
                Pending::FileStep {
                    device: id,
                    generation,
                    index: 0,
                    start: now,
                    stack: onward.clone(),
                },
            ),
            Some(_) => notes.extend(self.file_step(id, generation, 0, now, onward)),
            None => {}
        }
        notes
    }

    /// The notes of a MIDI file due together from `index`, and the next
    /// ones queued.
    fn file_step(
        &mut self,
        id: usize,
        generation: u64,
        index: usize,
        start: f64,
        onward: &Stack,
    ) -> Vec<Note> {
        let Node::MidiFile(file) = &self.graph.devices[id] else {
            return Vec::new();
        };
        let file = Arc::clone(file);
        let beat_ms = self.graph.beat_ms;
        let State::File {
            generation: playing,
            sounding,
        } = &mut self.states[id]
        else {
            return Vec::new();
        };
        if *playing != generation {
            return Vec::new();
        }
        let Some(first) = file.notes.get(index) else {
            return Vec::new();
        };
        let mut notes = Vec::new();
        let mut next = index;
        while let Some(event) = file.notes.get(next).filter(|e| e.beat == first.beat) {
            if event.velocity > 0 {
                sounding.push(event.note);
            } else if let Some(at) = sounding.iter().position(|&n| n == event.note) {
                sounding.swap_remove(at);
            }
            notes.push(Note {
                note: event.note,
                velocity: event.velocity,
                layer: 0,
            });
            next += 1;
        }
        if let Some(event) = file.notes.get(next) {
            self.later(
                start + event.beat * beat_ms,
                Pending::FileStep {
                    device: id,
                    generation,
                    index: next,
                    start,
                    stack: onward.clone(),
                },
            );
        }
        notes
    }

    /// The lights a Resonator makes of the pads pressed: those of now come
    /// back, the later ones are queued.
    fn resonate(
        &mut self,
        resonator: &Resonator,
        notes: &[Note],
        now: f64,
        onward: &Stack,
    ) -> Vec<Note> {
        let mut timeline: Vec<(f64, Note)> = Vec::new();
        for note in notes.iter().filter(|note| note.is_on()) {
            let Some((x, y)) = drum_rack::pad(note.note) else {
                continue;
            };
            let (x, y) = (i32::from(x), i32::from(y));
            // Every pad lit, with how far from the one pressed.
            let mut cells: Vec<((i32, i32), i32)> = vec![((x, y), 0)];
            for distance in 1..=i32::from(resonator.steps) {
                for &(dx, dy) in &resonator.directions {
                    cells.push((
                        (x + i32::from(dx) * distance, y + i32::from(dy) * distance),
                        distance,
                    ));
                }
                for &shape in &resonator.shapes {
                    for cy in 1..=8 {
                        for cx in 1..=8 {
                            if shape.on_ring(cx - x, cy - y, distance) {
                                cells.push(((cx, cy), distance));
                            }
                        }
                    }
                }
            }
            cells.retain(|&((cx, cy), _)| (1..=8).contains(&cx) && (1..=8).contains(&cy));
            cells.sort_by_key(|&(cell, distance)| (cell, distance));
            cells.dedup_by_key(|&mut (cell, _)| cell);

            let colours = resonator
                .gradient
                .clone()
                .unwrap_or_else(|| vec![note.velocity]);
            let colour_ms = f64::from(resonator.colour_ms.max(1.0));
            let once = colours.len() as f64 * colour_ms;
            let lasts = if resonator.gradient.is_none() {
                f64::from(resonator.length_ms)
            } else if resonator.loop_gradient {
                f64::from(resonator.length_ms).max(once)
            } else {
                once
            };
            for ((cx, cy), distance) in cells {
                let pad = drum_rack::note(cx as u8, cy as u8);
                let order = if resonator.inwards {
                    i32::from(resonator.steps) - distance
                } else {
                    distance
                };
                let start = now + f64::from(order) * f64::from(resonator.move_ms);
                let mut step = 0usize;
                while (step as f64) * colour_ms < lasts
                    && (resonator.loop_gradient || step < colours.len())
                {
                    let velocity = colours[step % colours.len()];
                    timeline.push((
                        start + step as f64 * colour_ms,
                        Note {
                            note: pad,
                            velocity,
                            layer: 0,
                        },
                    ));
                    step += 1;
                }
                timeline.push((
                    start + lasts,
                    Note {
                        note: pad,
                        velocity: 0,
                        layer: 0,
                    },
                ));
            }
        }
        timeline.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut now_notes = Vec::new();
        let mut at = 0;
        while at < timeline.len() {
            let time = timeline[at].0;
            let group: Vec<Note> = timeline[at..]
                .iter()
                .take_while(|(t, _)| *t == time)
                .map(|&(_, n)| n)
                .collect();
            at += group.len();
            if time <= now {
                now_notes.extend(group);
            } else {
                self.later(time, Pending::Notes(group, onward.clone()));
            }
        }
        now_notes
    }

    fn state(&mut self, id: usize, new: impl FnOnce() -> State) -> &mut State {
        if matches!(self.states[id], State::None) {
            self.states[id] = new();
        }
        &mut self.states[id]
    }

    /// Notes into an arpeggiator; what it plays at once comes back.
    fn arpeggiate(
        &mut self,
        id: usize,
        arp: Arpeggiator,
        notes: &[Note],
        now: f64,
        onward: &Stack,
    ) -> Vec<Note> {
        let State::Arpeggiator(state) = self.state(id, || State::Arpeggiator(ArpState::default()))
        else {
            unreachable!()
        };
        let mut started = false;
        for note in notes {
            if note.is_on() {
                // A chord after all were let go takes the place of the one
                // held on.
                if state.held.is_empty() {
                    state.notes.clear();
                }
                state.held.push(note.note);
                state.notes.retain(|&(n, _)| n != note.note);
                state.notes.push((note.note, note.velocity));
                started = true;
            } else {
                state.held.retain(|&n| n != note.note);
                if !arp.hold {
                    state.notes.retain(|&(n, _)| n != note.note);
                }
            }
        }
        state.notes.sort_unstable();
        if state.notes.is_empty() {
            state.running = false;
            state.generation += 1;
            return Vec::new();
        }
        if !started {
            return Vec::new();
        }
        // Each new chord plays its pattern from the start.
        state.generation += 1;
        state.running = true;
        state.step = 0;
        state.start = now;
        let generation = state.generation;
        self.step(id, generation, now, onward)
    }

    /// An arpeggiator's next note, if it still plays, and the one after
    /// queued.
    fn step(&mut self, id: usize, generation: u64, now: f64, onward: &Stack) -> Vec<Note> {
        let Node::Arpeggiator(arp) = self.graph.devices[id] else {
            return Vec::new();
        };
        let State::Arpeggiator(state) = &mut self.states[id] else {
            return Vec::new();
        };
        if !state.running || state.generation != generation || state.notes.is_empty() {
            return Vec::new();
        }
        let order = arp.style.order(state.notes.len());
        if arp
            .repeats
            .is_some_and(|repeats| state.step >= repeats as usize * order.len())
        {
            state.running = false;
            return Vec::new();
        }
        let (note, velocity) = state.notes[order[state.step % order.len()]];
        state.step += 1;
        let step_ms = f64::from(arp.step_ms).max(SHORTEST_STEP_MS);
        let next = state.start + state.step as f64 * step_ms;
        let more = arp
            .repeats
            .is_none_or(|repeats| state.step < repeats as usize * order.len());
        let end = now + step_ms * f64::from(arp.gate);
        self.later(
            end,
            Pending::Notes(
                vec![Note {
                    note,
                    velocity: 0,
                    layer: 0,
                }],
                onward.clone(),
            ),
        );
        if more {
            self.later(
                next,
                Pending::Step {
                    device: id,
                    generation,
                    stack: onward.clone(),
                },
            );
        } else if let State::Arpeggiator(state) = &mut self.states[id] {
            state.running = false;
        }
        vec![Note {
            note,
            velocity,
            layer: 0,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::midifx::{ArpStyle, Branch, Rack, VelocityMode};

    fn on(note: u8, velocity: u8) -> Note {
        Note {
            note,
            velocity,
            layer: 0,
        }
    }

    fn off(note: u8) -> Note {
        Note {
            note,
            velocity: 0,
            layer: 0,
        }
    }

    fn lights(chain: Chain) -> MidiFx {
        MidiFx::new(&MidiLights {
            chains: vec![chain],
            tempo: 120.0,
            page_notes: 100,
            palette: None,
        })
    }

    fn play(fx: &mut MidiFx, note: Note, at: f64) -> Vec<Note> {
        let mut out = Vec::new();
        fx.advance(at, &mut out);
        fx.input(note, at, &mut out);
        out
    }

    #[test]
    fn pitch_chord_velocity() {
        let mut fx = lights(vec![
            Device::Pitch {
                shift: 6,
                lowest: 0,
                range: 127,
            },
            Device::Chord {
                shifts: vec![(-1, 1.0), (3, 1.0)],
            },
            Device::Velocity(VelocityMap {
                lowest: 1,
                range: 127,
                out_low: 1,
                out_high: 94,
                mode: VelocityMode::Clip,
            }),
        ]);
        assert_eq!(
            play(&mut fx, on(65, 127), 0.0),
            [on(71, 94), on(70, 94), on(74, 94)]
        );
        // Held, then let go: the same notes end.
        assert_eq!(play(&mut fx, off(65), 500.0), [off(71), off(70), off(74)]);
    }

    #[test]
    fn note_length_ends_notes_itself() {
        let mut fx = lights(vec![Device::NoteLength {
            ms: 1500.0,
            on_release: false,
        }]);
        assert_eq!(play(&mut fx, on(40, 100), 0.0), [on(40, 100)]);
        assert!(play(&mut fx, off(40), 10.0).is_empty());
        assert_eq!(fx.next_due(), Some(1500.0));
        let mut out = Vec::new();
        fx.advance(1500.0, &mut out);
        assert_eq!(out, [off(40)]);
    }

    #[test]
    fn arpeggio_once_up() {
        // A chord of three, a note every 125 ms (a 16th at 120 BPM), each
        // two steps long, played once, held on.
        let mut fx = lights(vec![
            Device::Chord {
                shifts: vec![(4, 1.0), (8, 1.0)],
            },
            Device::Arpeggiator(Arpeggiator {
                style: ArpStyle::Up,
                step_ms: 125.0,
                gate: 2.0,
                repeats: Some(1),
                hold: true,
            }),
        ]);
        let mut timeline = Vec::new();
        timeline.extend(
            play(&mut fx, on(36, 100), 0.0)
                .into_iter()
                .map(|n| (0.0, n)),
        );
        // Let go at once: held on all the same.
        timeline.extend(play(&mut fx, off(36), 1.0).into_iter().map(|n| (1.0, n)));
        while let Some(at) = fx.next_due() {
            let mut out = Vec::new();
            fx.advance(at, &mut out);
            timeline.extend(out.into_iter().map(|n| (at, n)));
        }
        assert_eq!(
            timeline,
            [
                (0.0, on(36, 100)),
                (125.0, on(40, 100)),
                (250.0, off(36)),
                (250.0, on(44, 100)),
                (375.0, off(40)),
                (500.0, off(44)),
            ]
        );
    }

    #[test]
    fn midi_files_start_over() {
        use crate::model::midifx::{FileNote, MidiFile};
        // At 120 BPM, a beat is 500 ms.
        let file = MidiFile {
            notes: vec![
                FileNote {
                    beat: 0.5,
                    note: 36,
                    velocity: 3,
                },
                FileNote {
                    beat: 1.0,
                    note: 36,
                    velocity: 0,
                },
                FileNote {
                    beat: 1.0,
                    note: 37,
                    velocity: 5,
                },
                FileNote {
                    beat: 2.0,
                    note: 37,
                    velocity: 0,
                },
            ],
        };
        let mut fx = lights(vec![Device::MidiFile(Arc::new(file))]);
        // The note pressed goes no further; the file waits for its time.
        assert!(play(&mut fx, on(60, 127), 0.0).is_empty());
        assert_eq!(play(&mut fx, off(60), 100.0), []);
        let mut out = Vec::new();
        fx.advance(250.0, &mut out);
        assert_eq!(out, [on(36, 3)]);
        // Pressed again while its first note sounds: that note ends, and the
        // file plays from its start, the old run's notes dropped.
        assert_eq!(play(&mut fx, on(60, 127), 300.0), [off(36)]);
        let mut out = Vec::new();
        fx.advance(549.0, &mut out);
        assert!(out.is_empty(), "{out:?}");
        fx.advance(550.0, &mut out);
        assert_eq!(out, [on(36, 3)]);
    }

    #[test]
    fn resonator_lines() {
        use crate::model::midifx::Resonator;
        // From the pad at (2, 1), note 37: a line to the right, two pads
        // long, 10 ms a pad, each light white then red, 20 ms a colour.
        let resonator = Resonator {
            directions: vec![(1, 0)],
            shapes: Vec::new(),
            steps: 2,
            move_ms: 10.0,
            inwards: false,
            gradient: Some(vec![3, 5]),
            colour_ms: 20.0,
            length_ms: 100.0,
            loop_gradient: false,
        };
        let mut fx = lights(vec![Device::Resonator(Arc::new(resonator))]);
        assert_eq!(play(&mut fx, on(37, 127), 0.0), [on(37, 3)]);
        let mut timeline = Vec::new();
        while let Some(at) = fx.next_due() {
            let mut out = Vec::new();
            fx.advance(at, &mut out);
            timeline.extend(out.into_iter().map(|n| (at, n)));
        }
        // (3, 1) is note 38, (4, 1) note 39.
        assert_eq!(
            timeline,
            [
                (10.0, on(38, 3)),
                (20.0, on(37, 5)),
                (20.0, on(39, 3)),
                (30.0, on(38, 5)),
                (40.0, off(37)),
                (40.0, on(39, 5)),
                (50.0, off(38)),
                (60.0, off(39)),
            ]
        );
    }

    #[test]
    fn racks_by_key_and_page() {
        let branch = |keys: (u8, u8), selector: (f32, f32), shift: i32| Branch {
            keys,
            velocities: (1, 127),
            selector,
            chain: vec![Device::Pitch {
                shift,
                lowest: 0,
                range: 127,
            }],
        };
        let mut fx = lights(vec![Device::Rack(Rack {
            selector: Selector::Page {
                first: 0.0,
                last: 7.0,
                pages: 8,
            },
            branches: vec![
                branch((36, 36), (0.0, 0.0), 1),
                branch((36, 36), (1.0, 1.0), 2),
                branch((37, 37), (0.0, 1.0), 3),
            ],
        })]);
        assert_eq!(play(&mut fx, on(36, 100), 0.0), [on(37, 100)]);
        // The page turns while the pad is held: its end goes where its
        // start went.
        fx.set_page(1);
        assert_eq!(play(&mut fx, off(36), 10.0), [off(37)]);
        assert_eq!(play(&mut fx, on(36, 100), 20.0), [on(38, 100)]);
        assert_eq!(play(&mut fx, on(37, 100), 30.0), [on(40, 100)]);
    }
}
