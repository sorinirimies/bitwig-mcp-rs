//! MCP server that controls Bitwig Studio.
//!
//! Two backends:
//! * [`bridge`] (default): our own Bitwig extension over TCP. Full access.
//! * [`osc`]: the DrivenByMoss OSC extension. No extension of ours needed, but a bank window
//!   only and no note writing.
//!
//! Pure modules: [`config`], [`music`], [`midi`], [`hardware`], [`bridge::protocol`],
//! [`bridge::project`], [`osc::wire`], [`osc::state`].

pub mod bridge;
pub mod capture;
pub mod config;
pub mod extension;
/// Hardware synth profiles: the [`synthazia_hardware`] crate, shared with the Synthazia DAW.
pub use synthazia_hardware as hardware;
pub mod midi;
pub mod music;
pub mod osc;
pub mod smf;
pub mod theory;
