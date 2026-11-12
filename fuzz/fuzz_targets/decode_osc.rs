#![no_main]
//! Properties, not just "no panic": whatever Bitwig (or anything on the port) sends, decoding
//! never panics; every decoded message survives an encode/decode round trip with its address
//! intact; and feeding it to the state cache and rendering every view never panics.

use bitwig_mcp_rs::osc::wire::{Scale, decode, encode};
use bitwig_mcp_rs::osc::state::State;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut state = State::default();
    for msg in decode(data) {
        if let Ok(bytes) = encode(&msg.addr, msg.args.clone()) {
            let again = decode(&bytes);
            assert_eq!(again.len(), 1);
            assert_eq!(again[0].addr, msg.addr);
        }
        state.apply(&msg);
    }
    let _ = state.overview(Scale::new(128), 8);
    let _ = state.browser();
    let _ = state.with_prefix("/track/");
});
