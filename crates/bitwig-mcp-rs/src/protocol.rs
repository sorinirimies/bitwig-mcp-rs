//! OSC wire handling and value scaling. Pure.

use anyhow::{Context, Result};
use rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

/// Converts between normalized `0.0..=1.0` and the DrivenByMoss integer range `0..=MAX_VALUE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    max: i32,
}

impl Scale {
    /// `resolution` is the DrivenByMoss "Value resolution" setting (128 = values 0..=127).
    #[must_use]
    pub fn new(resolution: u32) -> Self {
        let max = i32::try_from(resolution.saturating_sub(1)).unwrap_or(i32::MAX);
        Self { max: max.max(1) }
    }

    /// `MAX_VALUE`.
    #[must_use]
    pub fn max(self) -> i32 {
        self.max
    }

    /// Normalized value to wire integer; out-of-range and NaN are clamped.
    #[must_use]
    pub fn to_wire(self, normalized: f64) -> i32 {
        let n = if normalized.is_nan() {
            0.0
        } else {
            normalized.clamp(0.0, 1.0)
        };
        // n is in 0..=1 and max fits i32, so the product fits i32.
        (n * f64::from(self.max)).round() as i32
    }

    /// Wire integer (or float) to normalized `0.0..=1.0`.
    #[must_use]
    pub fn from_wire(self, wire: f64) -> f64 {
        (wire / f64::from(self.max)).clamp(0.0, 1.0)
    }
}

/// Encodes one OSC message into a datagram.
///
/// # Errors
/// If the address or arguments cannot be encoded.
pub fn encode(addr: &str, args: Vec<OscType>) -> Result<Vec<u8>> {
    let packet = OscPacket::Message(OscMessage {
        addr: addr.to_owned(),
        args,
    });
    encoder::encode(&packet).with_context(|| format!("cannot encode OSC message {addr}"))
}

/// Decodes a datagram into its messages (bundles are flattened). Garbage gives no messages.
#[must_use]
pub fn decode(buf: &[u8]) -> Vec<OscMessage> {
    fn walk(packet: OscPacket, out: &mut Vec<OscMessage>) {
        match packet {
            OscPacket::Message(m) => out.push(m),
            OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| walk(p, out)),
        }
    }
    let mut out = Vec::new();
    // One datagram can hold one packet; trailing bytes are ignored.
    if let Ok((_, packet)) = decoder::decode_udp(buf) {
        walk(packet, &mut out);
    }
    out
}

/// Parses a user supplied OSC argument: `true`/`false`, integer, float, else text.
#[must_use]
pub fn parse_arg(raw: &str) -> OscType {
    match raw {
        "true" => OscType::Bool(true),
        "false" => OscType::Bool(false),
        _ => raw
            .parse::<i32>()
            .map(OscType::Int)
            .or_else(|_| raw.parse::<f32>().map(OscType::Float))
            .unwrap_or_else(|_| OscType::String(raw.to_owned())),
    }
}

/// `true` -> 1, `false` -> 0.
#[must_use]
pub fn flag(on: bool) -> OscType {
    OscType::Int(i32::from(on))
}

/// Validates an OSC address: starts with `/`, no spaces, no wildcard characters.
///
/// # Errors
/// A description of what is wrong.
pub fn check_address(addr: &str) -> Result<(), String> {
    if !addr.starts_with('/') {
        return Err(format!("OSC address must start with '/': {addr:?}"));
    }
    if addr.len() > 200 || addr.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("invalid OSC address: {addr:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_round_trip_and_clamp() {
        let s = Scale::new(128);
        assert_eq!(s.max(), 127);
        assert_eq!(s.to_wire(0.0), 0);
        assert_eq!(s.to_wire(1.0), 127);
        assert_eq!(s.to_wire(0.5), 64);
        assert_eq!(s.to_wire(-3.0), 0);
        assert_eq!(s.to_wire(9.0), 127);
        assert_eq!(s.to_wire(f64::NAN), 0);
        assert!((s.from_wire(127.0) - 1.0).abs() < 1e-9);
        assert_eq!(s.from_wire(1000.0), 1.0);
    }

    #[test]
    fn scale_follows_resolution() {
        assert_eq!(Scale::new(1024).to_wire(1.0), 1023);
        assert_eq!(Scale::new(0).max(), 1);
    }

    #[test]
    fn encode_decode_round_trip() {
        let bytes = encode("/track/1/volume", vec![OscType::Int(100)]).expect("encode");
        let msgs = decode(&bytes);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].addr, "/track/1/volume");
        assert_eq!(msgs[0].args, vec![OscType::Int(100)]);
    }

    #[test]
    fn decode_garbage_is_empty() {
        assert!(decode(b"not osc at all").is_empty());
        assert!(decode(&[]).is_empty());
    }

    #[test]
    fn parse_arg_types() {
        assert_eq!(parse_arg("true"), OscType::Bool(true));
        assert_eq!(parse_arg("12"), OscType::Int(12));
        assert_eq!(parse_arg("1.5"), OscType::Float(1.5));
        assert_eq!(parse_arg("play"), OscType::String("play".into()));
    }

    #[test]
    fn address_validation() {
        assert!(check_address("/play").is_ok());
        assert!(check_address("play").is_err());
        assert!(check_address("/a b").is_err());
        assert!(check_address("").is_err());
    }
}
