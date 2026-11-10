#![no_main]
//! Properties of the bridge framing and message parsing, on arbitrary bytes from the socket:
//! never panics, never returns more bytes than it was given, consumes whole frames only, and
//! a payload framed by us comes back unchanged.

use bitwig_mcp_rs::bridge::protocol::{Reply, Request, frame, take_frame, unwrap_payload};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut buf = data.to_vec();
    let mut taken = 0usize;
    while let Ok(Some(p)) = take_frame(&mut buf) {
        taken += p.len() + 4;
        assert!(taken <= data.len());
        let inner = unwrap_payload(&p);
        assert!(inner.len() <= p.len());
        let _ = serde_json::from_slice::<Reply>(inner);
        let _ = serde_json::from_slice::<Request>(inner);
    }
    assert_eq!(taken + buf.len(), data.len());

    let mut again = frame(data);
    assert_eq!(take_frame(&mut again), Ok(Some(data.to_vec())));
    assert!(again.is_empty());
});
