//! What each Launchpad model is told and tells: one profile a model.

mod mini_mk3;

pub use mini_mk3::MiniMk3;

use crate::model::{LedColor, LedTarget};

/// What a Launchpad says was pressed or let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceInput {
    /// A pad of the grid, by (x, y) from the bottom left, with how hard it
    /// was hit; let go when `velocity` is 0.
    Pad { x: u8, y: u8, velocity: u8 },
    /// A round button of the right side, 1 at the bottom.
    Side { n: u8, pressed: bool },
    /// A round button of the top row, 1 on the left.
    Top { n: u8, pressed: bool },
}

/// A Launchpad model.
pub trait Profile: Send + Sync {
    fn name(&self) -> &'static str;

    /// Whether a MIDI port, by the name the system gives it, is the one
    /// this model is driven through.
    fn matches_port(&self, port: &str) -> bool;

    /// What puts it in the layout PadWire speaks to it in.
    fn init_messages(&self) -> Vec<Vec<u8>>;

    /// What puts it back as it was, once PadWire lets go of it.
    fn exit_messages(&self) -> Vec<Vec<u8>>;

    fn decode(&self, message: &[u8]) -> Option<DeviceInput>;

    /// The message that lights a light, into `out`, which is cleared first.
    /// Nothing is written for a light this model does not have.
    fn led_message(&self, target: LedTarget, color: LedColor, out: &mut Vec<u8>);

    /// Every light this model has.
    fn targets(&self) -> Vec<LedTarget>;
}

/// Every model PadWire drives.
pub fn all() -> Vec<&'static dyn Profile> {
    vec![&MiniMk3]
}
