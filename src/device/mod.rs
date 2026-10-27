//! The Launchpad: found among the MIDI ports, connected, put in the layout
//! PadWire speaks to it in, and let go of as it was found.
//!
//! The ports are looked at every second, so a Launchpad plugged in or out
//! is noticed while PadWire runs.

pub mod layout;
pub mod profiles;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::engine::led::{LedCommand, LedHandle, LedSink};
use crate::model::{LedColor, LedTarget};
use profiles::{DeviceInput, Profile};

/// How often the ports are looked at.
const POLL: Duration = Duration::from_secs(1);
/// What PadWire is called among the MIDI clients.
const CLIENT: &str = "PadWire";

/// The Launchpad connected, which the lights are sent to.
struct Connected {
    output: MidiOutputConnection,
    profile: &'static dyn Profile,
    port: String,
    message: Vec<u8>,
}

impl Connected {
    fn light(&mut self, target: LedTarget, color: LedColor) {
        self.profile.led_message(target, color, &mut self.message);
        if !self.message.is_empty()
            && let Err(e) = self.output.send(&self.message)
        {
            log::debug!("a light was not sent to the Launchpad: {e}");
        }
    }

    fn clear(&mut self) {
        for target in self.profile.targets() {
            self.light(target, LedColor::Off);
        }
    }

    /// Put out its lights and give it back its own modes.
    fn let_go(mut self) {
        self.clear();
        for message in self.profile.exit_messages() {
            let _ = self.output.send(&message);
        }
        log::info!("let go of the {} on {}", self.profile.name(), self.port);
    }
}

/// Where the lights go when a Launchpad is connected.
#[derive(Clone, Default)]
pub struct DeviceOutput(Arc<Mutex<Option<Connected>>>);

impl DeviceOutput {
    fn lock(&self) -> MutexGuard<'_, Option<Connected>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl LedSink for DeviceOutput {
    fn changed(&mut self, changes: &[(LedTarget, LedColor)]) {
        if let Some(connected) = self.lock().as_mut() {
            for &(target, color) in changes {
                connected.light(target, color);
            }
        }
    }

    fn refresh(&mut self, lit: &[(LedTarget, LedColor)]) {
        if let Some(connected) = self.lock().as_mut() {
            connected.clear();
            for &(target, color) in lit {
                connected.light(target, color);
            }
        }
    }
}

/// What is told of the Launchpad.
pub trait DeviceListener: Send + Sync + 'static {
    fn input(&self, input: DeviceInput);
    /// A Launchpad connected, by its model's name, or none any more.
    fn connected(&self, model: Option<&'static str>);
}

/// Looks for a Launchpad, and keeps the one found connected.
pub struct DeviceManager {
    stop: Arc<AtomicBool>,
    port: Arc<Mutex<Option<String>>>,
    output: DeviceOutput,
    thread: Option<JoinHandle<()>>,
}

impl DeviceManager {
    /// Start looking, on `port` if given, for any Launchpad PadWire knows
    /// if not.
    pub fn start(
        output: DeviceOutput,
        leds: LedHandle,
        listener: Arc<dyn DeviceListener>,
        port: Option<String>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let port = Arc::new(Mutex::new(port));
        let thread = std::thread::Builder::new()
            .name("padwire-midi".into())
            .spawn({
                let (stop, port, output) = (stop.clone(), port.clone(), output.clone());
                move || watch(&stop, &port, &output, &leds, listener)
            })
            .expect("cannot start the MIDI thread");
        DeviceManager {
            stop,
            port,
            output,
            thread: Some(thread),
        }
    }

    /// Use this port from now on, or any Launchpad.
    pub fn set_port(&self, port: Option<String>) {
        *self.port.lock().unwrap_or_else(PoisonError::into_inner) = port;
    }

    /// Let go of the Launchpad, as it was found.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(connected) = self.output.lock().take() {
            connected.let_go();
        }
    }
}

impl Drop for DeviceManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The MIDI ports a Launchpad could be on, by name.
pub fn port_names() -> Vec<String> {
    let Ok(input) = MidiInput::new(CLIENT) else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok())
        .collect()
}

fn watch(
    stop: &AtomicBool,
    port: &Mutex<Option<String>>,
    output: &DeviceOutput,
    leds: &LedHandle,
    listener: Arc<dyn DeviceListener>,
) {
    // Kept for as long as the Launchpad is connected: dropping it stops
    // listening.
    let mut input: Option<MidiInputConnection<()>> = None;
    while !stop.load(Ordering::Relaxed) {
        let wanted = port.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let names = port_names();
        let current = output
            .lock()
            .as_ref()
            .map(|connected| connected.port.clone());
        match current {
            Some(current) => {
                let still_wanted = wanted.as_ref().is_none_or(|wanted| *wanted == current);
                if !names.contains(&current) || !still_wanted {
                    input = None;
                    if let Some(connected) = output.lock().take() {
                        if names.contains(&current) {
                            connected.let_go();
                        } else {
                            log::info!("the {} on {current} is gone", connected.profile.name());
                        }
                    }
                    listener.connected(None);
                }
            }
            None => {
                if let Some((name, profile)) = pick(&names, wanted.as_deref()) {
                    match connect(&name, profile, listener.clone()) {
                        Ok((connected_input, connected)) => {
                            log::info!("connected to the {} on {name}", profile.name());
                            input = Some(connected_input);
                            *output.lock() = Some(connected);
                            leds.send(LedCommand::Refresh);
                            listener.connected(Some(profile.name()));
                        }
                        Err(e) => log::warn!("cannot connect to {name}: {e}"),
                    }
                }
            }
        }
        std::thread::sleep(POLL);
    }
    drop(input);
}

/// The port to connect to, and the model on it.
fn pick(names: &[String], wanted: Option<&str>) -> Option<(String, &'static dyn Profile)> {
    let profiles = profiles::all();
    match wanted {
        Some(wanted) => {
            let name = names.iter().find(|name| *name == wanted)?;
            // A port picked by hand is taken for the first model PadWire
            // knows unless its name says which.
            let profile = profiles
                .iter()
                .find(|p| p.matches_port(name))
                .unwrap_or(&profiles[0]);
            Some((name.clone(), *profile))
        }
        None => names.iter().find_map(|name| {
            profiles
                .iter()
                .find(|p| p.matches_port(name))
                .map(|p| (name.clone(), *p))
        }),
    }
}

fn connect(
    name: &str,
    profile: &'static dyn Profile,
    listener: Arc<dyn DeviceListener>,
) -> Result<(MidiInputConnection<()>, Connected), String> {
    let midi_in = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
    let in_port = midi_in
        .ports()
        .into_iter()
        .find(|port| midi_in.port_name(port).is_ok_and(|n| n == name))
        .ok_or("the port is gone")?;
    let midi_out = MidiOutput::new(CLIENT).map_err(|e| e.to_string())?;
    let out_ports: Vec<_> = midi_out
        .ports()
        .into_iter()
        .filter_map(|port| Some((midi_out.port_name(&port).ok()?, port)))
        .collect();
    // The same port both ways, as a Launchpad has it; or else a port of the
    // same client the model would be on, as virtual ports come in pairs.
    let client = |port: &str| port.split(':').next().unwrap_or_default().to_owned();
    let out_port = out_ports
        .iter()
        .find(|(n, _)| n == name)
        .or_else(|| {
            out_ports
                .iter()
                .find(|(n, _)| client(n) == client(name) && profile.matches_port(n))
        })
        .map(|(_, port)| port)
        .ok_or("the port takes no messages")?;
    let mut output = midi_out
        .connect(out_port, "padwire-out")
        .map_err(|e| e.to_string())?;
    for message in profile.init_messages() {
        output.send(&message).map_err(|e| e.to_string())?;
    }
    let input = midi_in
        .connect(
            &in_port,
            "padwire-in",
            move |_, message, _| {
                if let Some(input) = profile.decode(message) {
                    listener.input(input);
                }
            },
            (),
        )
        .map_err(|e| e.to_string())?;
    Ok((
        input,
        Connected {
            output,
            profile,
            port: name.to_owned(),
            message: Vec::new(),
        },
    ))
}
