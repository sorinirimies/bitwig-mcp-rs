//! Backend 1 (default): our own Bitwig extension (`bitwig-bridge/`) over TCP.
//!
//! Full project access: every track, exact clip notes, devices by name, all remote-control
//! pages, application actions.

pub mod client;
pub mod project;
pub mod protocol;
pub mod server;
