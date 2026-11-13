//! External hardware: playing notes, raw MIDI, SysEx, instrument profiles.

use super::super::*;

/// Notes to audition.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlayNotesArgs {
    /// Track name or 1-based number (a HW Instrument track plays the hardware synth).
    pub track: Ref,
    /// Notes (numbers, names like "C3", or the instrument's named keys like "kick"); several =
    /// a chord.
    pub notes: Vec<NoteRef>,
    /// Instrument profile whose named keys (drum map) the notes may use.
    pub instrument: Option<String>,
    /// Velocity 1-127 (default 100).
    pub velocity: Option<u8>,
    /// Hold time in ms (default 500, max 10000).
    pub duration_ms: Option<u32>,
}

/// Raw MIDI.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendMidiArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Messages as [status, data1, data2], e.g. [[176, 74, 64]] = CC 74 on channel 1.
    pub messages: Vec<[u8; 3]>,
}

/// A hardware parameter.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HardwareSetArgs {
    /// Track with the HW Instrument device.
    pub track: Ref,
    /// Instrument profile name (see list_instruments); default "generic".
    pub instrument: Option<String>,
    /// Parameter name from the profile, e.g. "cutoff".
    pub param: String,
    /// Value 0.0-1.0 over the parameter's range.
    pub value: Option<f64>,
    /// Bipolar value -1.0..1.0 instead (0 = center), for centered parameters.
    pub bipolar: Option<f64>,
    /// Raw MIDI value instead, within the parameter's range.
    pub raw: Option<u16>,
    /// Named step instead, for parameters with named values.
    pub named: Option<String>,
    /// MIDI channel 1-16; default: the profile's.
    pub channel: Option<u8>,
}

/// A raw SysEx message.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendSysexArgs {
    /// Hex bytes, e.g. "F0 00 20 32 00 7F 0A 03 00 0C F7".
    pub hex: String,
}

/// A program change.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProgramChangeArgs {
    /// Track with the HW Instrument device.
    pub track: Ref,
    /// Program 0-127.
    pub program: u8,
    /// Bank select MSB (CC 0).
    pub bank_msb: Option<u8>,
    /// Bank select LSB (CC 32).
    pub bank_lsb: Option<u8>,
    /// MIDI channel 1-16 (default 1).
    pub channel: Option<u8>,
}

/// Parses hex SysEx (`"F0 00 20 32 F7"`, spaces optional) and checks the `F0 … F7` frame.
///
/// # Errors
/// Bad hex, a missing frame, data bytes above 7F, or more than 4096 bytes.
pub(crate) fn parse_sysex(hex: &str) -> Result<Vec<u8>, String> {
    let digits: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return Err("SysEx hex needs two digits per byte".into());
    }
    let bytes = (0..digits.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&digits[i..i + 2], 16)
                .map_err(|_| format!("bad hex byte {:?}", &digits[i..i + 2]))
        })
        .collect::<Result<Vec<u8>, _>>()?;
    match bytes.as_slice() {
        [0xF0, inner @ .., 0xF7] if !inner.is_empty() && inner.len() < 4096 => {
            if inner.iter().any(|b| *b > 0x7F) {
                return Err("SysEx data bytes must be 00-7F".into());
            }
            Ok(bytes)
        }
        _ => Err("SysEx must start with F0, end with F7 and hold 1-4095 data bytes".into()),
    }
}

#[tool_router(router = hardware_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Play notes on a track for a while (several notes = chord). On a HW Instrument track this plays the external synth."
    )]
    pub(crate) async fn play_notes(&self, Parameters(a): Parameters<PlayNotesArgs>) -> ToolResult {
        if a.notes.is_empty() {
            return Err(bad("notes is empty"));
        }
        let velocity = a.velocity.unwrap_or(100);
        if velocity == 0 || velocity > 127 {
            return Err(bad("velocity must be 1-127"));
        }
        let reg = a.instrument.as_ref().map(|_| self.profiles());
        let profile = match (&reg, a.instrument.as_deref()) {
            (Some(reg), Some(name)) => Some(Self::profile(reg, name)?),
            _ => None,
        };
        let keys = a
            .notes
            .iter()
            .map(|n| match (n, profile) {
                (NoteRef::Name(name), Some(p)) => match p.key(name) {
                    Some(k) => Ok(k),
                    None => n.key(),
                },
                _ => n.key(),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (_, track) = self.track_of(&a.track).await?;
        let duration_ms = a.duration_ms.unwrap_or(500).min(10_000);
        out(&self
            .call(Command::PlayNotes {
                track,
                keys,
                velocity,
                duration_ms,
            })
            .await?)
    }

    #[tool(
        description = "Send raw 3-byte MIDI messages into a track (e.g. on to a hardware synth through its HW Instrument device)."
    )]
    pub(crate) async fn send_midi(&self, Parameters(a): Parameters<SendMidiArgs>) -> ToolResult {
        if a.messages.is_empty() || a.messages.len() > 256 {
            return Err(bad("send 1-256 messages"));
        }
        if a.messages
            .iter()
            .any(|m| m[0] < 0x80 || m[1] > 127 || m[2] > 127)
        {
            return Err(bad(
                "each message is [status 128-255, data1 0-127, data2 0-127]",
            ));
        }
        let (_, track) = self.track_of(&a.track).await?;
        out(&self
            .call(Command::SendMidi {
                track,
                messages: a.messages,
            })
            .await?)
    }

    #[tool(
        description = "Hardware instrument profiles: built-in ones (generic General MIDI with a GM drum map; Behringer Model D, Crave, CAT, Wasp, Edge, Solina, 2600) plus <config dir>/bitwig-mcp-rs/instruments/*.toml (the 'dir' it reports). Each lists its MIDI channel, what it can do over MIDI (notes), its named parameters (CC, NRPN, RPN, pitch bend, pressure, SysEx; with range and named values) and named keys."
    )]
    pub(crate) async fn list_instruments(&self) -> ToolResult {
        let reg = self.profiles();
        let errors: Vec<String> = reg.errors().iter().map(ToString::to_string).collect();
        out(&json!({
            "dir": self.profiles_dir.display().to_string(),
            "instruments": reg.profiles(),
            "errors": errors,
        }))
    }

    #[tool(
        description = "Set a named parameter of an external synth from its instrument profile: CC/NRPN/RPN/pitch bend go through the track's HW Instrument device, SysEx (e.g. Model D settings) through the MIDI output assigned to the MCP bridge controller. Give value (0-1), bipolar (-1..1), raw, or named."
    )]
    pub(crate) async fn hardware_set(
        &self,
        Parameters(a): Parameters<HardwareSetArgs>,
    ) -> ToolResult {
        let reg = self.profiles();
        let profile = Self::profile(&reg, a.instrument.as_deref().unwrap_or("generic"))?;
        let value = match (a.value, a.bipolar, a.raw, a.named) {
            (Some(v), None, None, None) => hardware::Value::Normalized(v),
            (None, Some(b), None, None) => hardware::Value::Bipolar(b),
            (None, None, Some(r), None) => hardware::Value::Raw(r),
            (None, None, None, Some(n)) => hardware::Value::Named(n),
            _ => return Err(bad("give exactly one of value, bipolar, raw, named")),
        };
        let messages = profile
            .messages(&a.param, &value, a.channel)
            .map_err(|e| bad(e.to_string()))?;
        let name = profile.param(&a.param).map(|(n, _)| n.to_owned());
        let (_, track) = self.track_of(&a.track).await?;
        self.send_messages(track, &messages).await?;
        let sent: Vec<String> = messages.iter().map(MidiMessage::hex).collect();
        out(&json!({"instrument": profile.name, "param": name, "sent": sent}))
    }

    #[tool(
        description = "Send a program change (with optional bank select) to an external synth through the track's HW Instrument device."
    )]
    pub(crate) async fn hardware_program_change(
        &self,
        Parameters(a): Parameters<ProgramChangeArgs>,
    ) -> ToolResult {
        let messages =
            hardware::program_change(a.channel.unwrap_or(1), a.program, a.bank_msb, a.bank_lsb)
                .map_err(|e| bad(e.to_string()))?;
        let (_, track) = self.track_of(&a.track).await?;
        self.send_messages(track, &messages).await?;
        let sent: Vec<String> = messages.iter().map(MidiMessage::hex).collect();
        out(&json!({"sent": sent}))
    }

    #[tool(
        description = "Send a raw SysEx message (hex, F0 ... F7) through the MIDI output assigned to the MCP bridge controller in Bitwig's settings, e.g. synth settings or patch requests."
    )]
    pub(crate) async fn send_sysex(&self, Parameters(a): Parameters<SendSysexArgs>) -> ToolResult {
        let bytes = parse_sysex(&a.hex).map_err(bad)?;
        out(&self.call(Command::SendSysex { bytes }).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn hardware_set_uses_the_generic_profile() {
        let (s, log) = rig().await;
        s.hardware_set(Parameters(HardwareSetArgs {
            track: Ref::Name("synth".into()),
            instrument: None,
            param: "cutoff".into(),
            value: Some(1.0),
            bipolar: None,
            raw: None,
            named: None,
            channel: Some(2),
        }))
        .await
        .expect("tool");
        let req = &sent(&log, "send_midi")[0];
        assert_eq!(req["messages"], json!([[0xB1, 74, 127]]));
        assert_eq!(req["track"]["index"], 1);
    }

    #[tokio::test]
    async fn hardware_sysex_settings_go_through_the_midi_output() {
        let (s, log) = rig().await;
        let r = s
            .hardware_set(Parameters(HardwareSetArgs {
                track: Ref::Name("synth".into()),
                instrument: Some("model d".into()),
                param: "bend_range".into(),
                value: None,
                bipolar: None,
                raw: Some(7),
                named: None,
                channel: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("F0 00 20 32 00 7F 0A 03 00 07 F7"), "{r}");
        let req = &sent(&log, "send_sysex")[0];
        assert_eq!(
            req["bytes"],
            json!([
                0xF0, 0x00, 0x20, 0x32, 0x00, 0x7F, 0x0A, 0x03, 0x00, 0x07, 0xF7
            ])
        );
        assert!(sent(&log, "send_midi").is_empty());
    }

    #[tokio::test]
    async fn play_notes_resolves_named_keys() {
        let (s, log) = rig().await;
        s.play_notes(Parameters(PlayNotesArgs {
            track: Ref::Number(1),
            notes: vec![NoteRef::Name("kick".into()), NoteRef::Name("C3".into())],
            instrument: Some("generic".into()),
            velocity: None,
            duration_ms: Some(10),
        }))
        .await
        .expect("tool");
        assert_eq!(sent(&log, "play_notes")[0]["keys"], json!([36, 60]));
    }

    #[tokio::test]
    async fn send_sysex_checks_the_frame() {
        let (s, log) = rig().await;
        s.send_sysex(Parameters(SendSysexArgs {
            hex: "f0 7d 01 f7".into(),
        }))
        .await
        .expect("tool");
        assert_eq!(
            sent(&log, "send_sysex")[0]["bytes"],
            json!([0xF0, 0x7D, 1, 0xF7])
        );
        assert!(
            s.send_sysex(Parameters(SendSysexArgs {
                hex: "90 3C 7F".into()
            }))
            .await
            .is_err()
        );
    }

    #[test]
    fn sysex_parsing() {
        assert_eq!(parse_sysex("F07D01F7"), Ok(vec![0xF0, 0x7D, 1, 0xF7]));
        assert!(parse_sysex("F0 F7").is_err(), "empty");
        assert!(parse_sysex("F0 80 F7").is_err(), "data above 7F");
        assert!(parse_sysex("F0 7").is_err(), "odd digits");
        assert!(parse_sysex("F0 ZZ F7").is_err(), "not hex");
    }
}
