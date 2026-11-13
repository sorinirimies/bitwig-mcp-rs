//! The tools, one module per area; each registers its own router, combined by `BridgeServer::all_tools` (in `server/mod.rs`).

pub mod clips;
pub mod compose;
pub mod devices;
pub mod export;
pub mod hardware;
pub mod studio;
pub mod tracks;
pub mod transport;
