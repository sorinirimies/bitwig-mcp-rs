//! MCP server that controls Bitwig Studio over OSC, through the
//! [DrivenByMoss](https://www.mossgrabers.de/Software/Bitwig/Bitwig.html) "Open Sound Control"
//! controller extension.
//!
//! Layers, bottom up:
//! * [`config`]: command line + environment settings (pure).
//! * [`protocol`]: OSC encode/decode and value scaling (pure).
//! * [`state`]: the cache of every value Bitwig reports, plus the JSON overview (pure).
//! * [`bitwig`]: UDP sockets: sends commands, keeps the state cache fresh.
//! * [`server`]: the MCP tools; each one maps to one or more OSC commands.

pub mod bitwig;
pub mod config;
pub mod midi;
pub mod protocol;
pub mod server;
pub mod state;
