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
pub mod config;
pub mod hardware;
pub mod midi;
pub mod music;
pub mod osc;
