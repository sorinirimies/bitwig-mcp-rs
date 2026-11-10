//! Backend 2: the DrivenByMoss "Open Sound Control" extension (`--backend osc`).
//!
//! Works without our own Bitwig extension, but only sees a bank window of tracks and
//! cannot write clip notes. See [`crate::bridge`] for the full backend.

pub mod client;
pub mod server;
pub mod state;
pub mod wire;
