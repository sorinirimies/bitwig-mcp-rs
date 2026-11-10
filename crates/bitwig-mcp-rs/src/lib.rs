//! Rust MCP server that controls Bitwig Studio over OSC (DrivenByMoss)
//!
//! Keep the logic here, pure and testable (take data and a `now`, return data);
//! `main.rs` only wires the outside world (args, files, clock) to it.

/// Splits a `key = value` line. Comments (`#`) and blank lines give `None`.
#[must_use]
pub fn parse_kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (k, v) = line.split_once('=')?;
    let k = k.trim();
    (!k.is_empty()).then(|| (k, v.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_trims() {
        assert_eq!(parse_kv(" a = b "), Some(("a", "b")));
        assert_eq!(parse_kv("a=b=c"), Some(("a", "b=c")));
    }

    #[test]
    fn skips_comments_blanks_and_empty_keys() {
        assert_eq!(parse_kv("# a = b"), None);
        assert_eq!(parse_kv("   "), None);
        assert_eq!(parse_kv(" = b"), None);
        assert_eq!(parse_kv("no equals"), None);
    }
}
