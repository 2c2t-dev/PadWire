//! PadWire: Launchpad covers played on Linux, without Ableton.
//!
//! Covers are read from their formats into one [`model`], which the
//! [`engine`] plays, its lights going to the Launchpad of [`device`] and to
//! the window.

pub mod config;
pub mod device;
pub mod engine;
pub mod formats;
pub mod model;
