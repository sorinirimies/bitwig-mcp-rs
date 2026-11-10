#![no_main]
//! Properties, not just "no panic": a parsed key is non-empty and trimmed, and
//! re-parsing `key = value` gives the same pair back.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(line) = std::str::from_utf8(data) else { return };
    if let Some((k, v)) = bitwig_mcp_rs::parse_kv(line) {
        assert!(!k.is_empty());
        assert_eq!(k, k.trim());
        assert_eq!(v, v.trim());
        assert_eq!(bitwig_mcp_rs::parse_kv(&format!("{k} = {v}")), Some((k, v)));
    }
});
