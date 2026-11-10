//! Hardware instrument profiles: named parameters of an external synth mapped to MIDI.
//! Pure, except [`load_dir`].
//!
//! The MIDI is sent *into a Bitwig track* (bridge `send_midi`); a "HW Instrument" device on that
//! track forwards it to the synth, so Bitwig keeps exclusive use of the MIDI port.
//!
//! A profile is a TOML file in the profiles directory (`--instruments-dir`, default
//! `~/.config/bitwig-mcp-rs/instruments`):
//!
//! ```toml
//! name = "Minilogue XD"
//! channel = 1                  # default MIDI channel 1-16
//!
//! [params.cutoff]
//! cc = 43                      # 7-bit CC
//! [params.resonance]
//! cc = 44
//! [params.fine_pitch]
//! cc14 = 1                     # 14-bit CC pair: MSB on cc, LSB on cc + 32
//! [params.lfo_shape]
//! nrpn = 1033                  # NRPN number 0-16383 (CC 99/98, data CC 6/38)
//! bits = 7                     # 7 (default) or 14 data bits
//! [params.osc1_wave]
//! cc = 50
//! values = ["saw", "triangle", "square"]   # named steps, spread evenly over 0-127
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bridge::protocol::Midi;

/// How one parameter is sent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Param {
    /// 7-bit control change number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<u8>,
    /// 14-bit CC: MSB on this number (0-31), LSB on number + 32.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc14: Option<u8>,
    /// NRPN number 0-16383.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nrpn: Option<u16>,
    /// Data resolution for NRPN: 7 or 14 bits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u8>,
    /// Named discrete values, evenly spread over the range.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Free text shown to the model (e.g. units).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One instrument.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Profile {
    /// Display name, also the lookup key (case-insensitive).
    pub name: String,
    /// Default MIDI channel 1-16.
    #[serde(default = "one")]
    pub channel: u8,
    /// Parameters by name.
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
}

fn one() -> u8 {
    1
}

/// A parameter value: normalized, raw, or one of the named steps.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// 0.0..=1.0.
    Normalized(f64),
    /// Raw MIDI value in the parameter's range.
    Raw(u16),
    /// Name from `values`.
    Named(String),
}

impl Profile {
    /// Parses and validates a profile.
    ///
    /// # Errors
    /// TOML errors and invalid parameter definitions.
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        let p: Self = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        p.validate()?;
        Ok(p)
    }

    fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.channel) {
            return Err(format!("{}: channel must be 1-16", self.name));
        }
        for (name, p) in &self.params {
            let kinds = [p.cc.is_some(), p.cc14.is_some(), p.nrpn.is_some()];
            if kinds.iter().filter(|k| **k).count() != 1 {
                return Err(format!(
                    "{}.{name}: set exactly one of cc, cc14, nrpn",
                    self.name
                ));
            }
            if p.cc.is_some_and(|c| c > 127) {
                return Err(format!("{}.{name}: cc must be 0-127", self.name));
            }
            if p.cc14.is_some_and(|c| c > 31) {
                return Err(format!("{}.{name}: cc14 must be 0-31", self.name));
            }
            if p.nrpn.is_some_and(|n| n > 16383) {
                return Err(format!("{}.{name}: nrpn must be 0-16383", self.name));
            }
            if p.bits.is_some_and(|b| b != 7 && b != 14) {
                return Err(format!("{}.{name}: bits must be 7 or 14", self.name));
            }
        }
        Ok(())
    }

    /// Looks a parameter up by name (case-insensitive; `-`, `_` and spaces are equal).
    #[must_use]
    pub fn param(&self, name: &str) -> Option<(&str, &Param)> {
        let key = normalize(name);
        self.params
            .iter()
            .find(|(k, _)| normalize(k) == key)
            .map(|(k, p)| (k.as_str(), p))
    }
}

fn normalize(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == '-' || c == ' ' { '_' } else { c })
        .collect()
}

impl Param {
    fn max(&self) -> u16 {
        if self.cc14.is_some() || (self.nrpn.is_some() && self.bits == Some(14)) {
            16383
        } else {
            127
        }
    }

    fn resolve(&self, value: &Value) -> Result<u16, String> {
        let max = self.max();
        match value {
            Value::Normalized(v) => {
                if !v.is_finite() {
                    return Err("value must be a number".into());
                }
                // clamped to 0..=1 and max <= 16383: fits u16
                Ok((v.clamp(0.0, 1.0) * f64::from(max)).round() as u16)
            }
            Value::Raw(r) if *r <= max => Ok(*r),
            Value::Raw(r) => Err(format!("raw value {r} exceeds {max}")),
            Value::Named(n) => {
                let i = self
                    .values
                    .iter()
                    .position(|v| v.eq_ignore_ascii_case(n))
                    .ok_or_else(|| format!("unknown value {n:?}; one of {:?}", self.values))?;
                let steps = self.values.len().saturating_sub(1).max(1);
                // i < len <= u16 range in practice; the result is within 0..=max
                Ok(((i as f64 / steps as f64) * f64::from(max)).round() as u16)
            }
        }
    }

    /// MIDI messages that set this parameter.
    ///
    /// # Errors
    /// Bad channel or value.
    pub fn messages(&self, channel: u8, value: &Value) -> Result<Vec<Midi>, String> {
        if !(1..=16).contains(&channel) {
            return Err("channel must be 1-16".into());
        }
        let status = 0xB0 | (channel - 1);
        let v = self.resolve(value)?;
        let (msb, lsb) = ((v >> 7) as u8, (v & 0x7F) as u8);
        if let Some(cc) = self.cc {
            return Ok(vec![[status, cc, v as u8]]);
        }
        if let Some(cc) = self.cc14 {
            return Ok(vec![[status, cc, msb], [status, cc + 32, lsb]]);
        }
        let n = self.nrpn.ok_or("parameter has no MIDI mapping")?;
        let mut out = vec![[status, 99, (n >> 7) as u8], [status, 98, (n & 0x7F) as u8]];
        if self.bits == Some(14) {
            out.push([status, 6, msb]);
            out.push([status, 38, lsb]);
        } else {
            out.push([status, 6, v as u8]);
        }
        Ok(out)
    }
}

/// Program change, optionally preceded by bank select (CC 0 MSB / CC 32 LSB).
///
/// # Errors
/// Out-of-range numbers.
pub fn program_change(
    channel: u8,
    program: u8,
    bank_msb: Option<u8>,
    bank_lsb: Option<u8>,
) -> Result<Vec<Midi>, String> {
    if !(1..=16).contains(&channel) {
        return Err("channel must be 1-16".into());
    }
    if program > 127 || bank_msb.is_some_and(|b| b > 127) || bank_lsb.is_some_and(|b| b > 127) {
        return Err("program and bank must be 0-127".into());
    }
    let ch = channel - 1;
    let mut out = Vec::new();
    if let Some(b) = bank_msb {
        out.push([0xB0 | ch, 0, b]);
    }
    if let Some(b) = bank_lsb {
        out.push([0xB0 | ch, 32, b]);
    }
    // Program change has one data byte; the bridge ignores the third.
    out.push([0xC0 | ch, program, 0]);
    Ok(out)
}

/// The built-in profile: General MIDI controllers most synths understand.
#[must_use]
pub fn generic() -> Profile {
    let cc = |n: u8, d: &str| Param {
        cc: Some(n),
        cc14: None,
        nrpn: None,
        bits: None,
        values: Vec::new(),
        description: Some(d.into()),
    };
    let params = [
        ("mod_wheel", cc(1, "modulation wheel")),
        ("breath", cc(2, "breath controller")),
        ("volume", cc(7, "channel volume")),
        ("pan", cc(10, "pan, 0.5 = center")),
        ("expression", cc(11, "expression")),
        ("sustain", cc(64, "sustain pedal, >= 0.5 = on")),
        ("resonance", cc(71, "filter resonance / timbre")),
        ("release", cc(72, "release time")),
        ("attack", cc(73, "attack time")),
        ("cutoff", cc(74, "filter cutoff / brightness")),
        ("decay", cc(75, "decay time")),
        ("vibrato_rate", cc(76, "vibrato rate")),
        ("vibrato_depth", cc(77, "vibrato depth")),
        ("reverb", cc(91, "reverb send")),
        ("chorus", cc(93, "chorus send")),
        ("all_notes_off", cc(123, "send 0 to stop hanging notes")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    Profile {
        name: "generic".into(),
        channel: 1,
        params,
    }
}

/// Loads every `*.toml` in `dir` plus the built-in generic profile. Broken files are reported,
/// not fatal.
#[must_use]
pub fn load_dir(dir: &Path) -> (Vec<Profile>, Vec<String>) {
    let mut profiles = vec![generic()];
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (profiles, errors);
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| Profile::parse(&t))
        {
            Ok(p) => profiles.push(p),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    (profiles, errors)
}

/// Finds a profile by name (case-insensitive).
#[must_use]
pub fn find<'a>(profiles: &'a [Profile], name: &str) -> Option<&'a Profile> {
    profiles
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name = "Test Synth"
channel = 2
[params.cutoff]
cc = 43
[params.fine]
cc14 = 1
[params.shape]
nrpn = 1033
[params.wide]
nrpn = 300
bits = 14
[params.wave]
cc = 50
values = ["saw", "triangle", "square"]
"#;

    #[test]
    fn parses_and_maps_cc() {
        let p = Profile::parse(SAMPLE).expect("parse");
        assert_eq!(p.channel, 2);
        let (name, cut) = p.param("Cutoff").expect("param");
        assert_eq!(name, "cutoff");
        assert_eq!(
            cut.messages(2, &Value::Normalized(1.0)),
            Ok(vec![[0xB1, 43, 127]])
        );
        assert_eq!(cut.messages(2, &Value::Raw(64)), Ok(vec![[0xB1, 43, 64]]));
        assert!(cut.messages(2, &Value::Raw(200)).is_err());
        assert!(cut.messages(0, &Value::Raw(1)).is_err());
    }

    #[test]
    fn maps_14_bit_and_nrpn() {
        let p = Profile::parse(SAMPLE).expect("parse");
        let fine = p.param("fine").expect("fine").1;
        assert_eq!(
            fine.messages(1, &Value::Raw(16383)),
            Ok(vec![[0xB0, 1, 127], [0xB0, 33, 127]])
        );
        let shape = p.param("shape").expect("shape").1;
        assert_eq!(
            shape.messages(1, &Value::Raw(5)),
            Ok(vec![[0xB0, 99, 8], [0xB0, 98, 9], [0xB0, 6, 5]])
        );
        let wide = p.param("wide").expect("wide").1;
        assert_eq!(wide.messages(1, &Value::Raw(200)).map(|m| m.len()), Ok(4));
    }

    #[test]
    fn named_values_spread_evenly() {
        let p = Profile::parse(SAMPLE).expect("parse");
        let wave = p.param("wave").expect("wave").1;
        assert_eq!(
            wave.messages(1, &Value::Named("SAW".into())),
            Ok(vec![[0xB0, 50, 0]])
        );
        assert_eq!(
            wave.messages(1, &Value::Named("triangle".into())),
            Ok(vec![[0xB0, 50, 64]])
        );
        assert_eq!(
            wave.messages(1, &Value::Named("square".into())),
            Ok(vec![[0xB0, 50, 127]])
        );
        assert!(wave.messages(1, &Value::Named("noise".into())).is_err());
    }

    #[test]
    fn rejects_bad_profiles() {
        assert!(Profile::parse("name = 'x'\nchannel = 17").is_err());
        assert!(Profile::parse("name = 'x'\n[params.a]\ncc = 1\nnrpn = 2").is_err());
        assert!(Profile::parse("name = 'x'\n[params.a]\nbits = 7").is_err());
        assert!(Profile::parse("name = 'x'\n[params.a]\ncc14 = 40").is_err());
        assert!(Profile::parse("not toml =").is_err());
    }

    #[test]
    fn program_change_with_bank() {
        assert_eq!(
            program_change(3, 10, Some(1), Some(2)),
            Ok(vec![[0xB2, 0, 1], [0xB2, 32, 2], [0xC2, 10, 0]])
        );
        assert!(program_change(1, 128, None, None).is_err());
    }

    #[test]
    fn generic_is_valid_and_loads_from_a_dir() {
        assert!(generic().validate().is_ok());
        let dir = std::env::temp_dir().join(format!("bwmcp-prof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("a.toml"), SAMPLE).expect("write");
        std::fs::write(dir.join("broken.toml"), "nope =").expect("write");
        std::fs::write(dir.join("ignored.txt"), "x").expect("write");
        let (profiles, errors) = load_dir(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(profiles.len(), 2);
        assert!(find(&profiles, "test synth").is_some());
        assert_eq!(errors.len(), 1);
        let (only_generic, none) = load_dir(Path::new("/definitely/not/here"));
        assert_eq!((only_generic.len(), none.len()), (1, 0));
    }
}
