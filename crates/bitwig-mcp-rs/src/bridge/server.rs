//! The MCP server on the bridge backend: ~30 tools, resources and prompts.
//!
//! Tools take names or 1-based numbers for tracks, scenes, slots, devices, sends and
//! parameters, resolve them against a fresh project snapshot, and send 0-based wire
//! addresses. Every change replies with Bitwig's state after it, so the model sees the result
//! (including Bitwig's own display text like "-6.0 dB").

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Implementation, ListResourcesResult, PaginatedRequestParams, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router,
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::client::Bridge;
use super::project::{ProjectView, Ref, TrackView};
use super::protocol::{
    ClipAction, ClipChange, Command, DeviceAction, DeviceSource, GridNote, Insert, Rgb,
    SceneAction, SendLevel, TrackAction, TrackAddr, TrackChange, TrackKind, TransportAction,
    TransportChange,
};
use crate::capture;
use crate::hardware::{self, MidiMessage, Profile, Registry};
use crate::music;
use crate::theory;
use std::path::Path;

type ToolResult = Result<String, McpError>;

/// How long to wait before resolving a name, clip or device again, and how often (seen
/// live: a new track needed over 600 ms to show up).
const SETTLE_RETRY: Duration = Duration::from_millis(500);
const SETTLE_ATTEMPTS: u32 = 3;

fn err(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn bad(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

fn out(v: &Json) -> ToolResult {
    serde_json::to_string(v).map_err(err)
}

const INSTRUCTIONS: &str = "Controls Bitwig Studio through the bitwig-mcp-rs bridge extension. \
Start with get_project. Tracks, scenes, slots, devices, sends and parameters are addressed by \
name (case-insensitive, unique substring is enough) or by 1-based number; 'master' is the \
master track and 'fx 1' the first effect track. Levels are 0.0-1.0; volumes can also be given \
in dB. Notes are numbers (60 = C3) or names like 'C3', 'F#2'; times are in beats (quarter \
notes). Changes reply with the state after the change. Build songs with create_track (with a \
device), create_scene, create_clip (with notes). External synths: put a HW Instrument device on \
a track, then use play_notes / hardware_set / hardware_program_change on that track. \
list_actions + invoke_action reach every Bitwig menu command (save, export, ...).";

// ---------------------------------------------------------------- pure helpers

/// Parses a color: `#rrggbb`, `rrggbb`, or a basic name.
///
/// # Errors
/// Unknown formats.
pub fn parse_color(s: &str) -> Result<Rgb, String> {
    let named = match s.trim().to_lowercase().as_str() {
        "red" => Some(Rgb(217, 46, 36)),
        "orange" => Some(Rgb(255, 131, 62)),
        "yellow" => Some(Rgb(228, 183, 78)),
        "green" => Some(Rgb(115, 152, 20)),
        "teal" | "cyan" => Some(Rgb(0, 166, 148)),
        "blue" => Some(Rgb(0, 153, 217)),
        "purple" | "violet" => Some(Rgb(149, 73, 203)),
        "pink" | "magenta" => Some(Rgb(217, 56, 113)),
        "brown" => Some(Rgb(163, 121, 67)),
        "grey" | "gray" => Some(Rgb(127, 127, 127)),
        "white" => Some(Rgb(255, 255, 255)),
        "black" => Some(Rgb(0, 0, 0)),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(c);
    }
    let hex = s.trim().trim_start_matches('#');
    let byte = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
    };
    match (hex.len(), byte(0), byte(2), byte(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok(Rgb(r, g, b)),
        _ => Err(format!(
            "not a color: {s:?} (use #rrggbb or a name like red, blue)"
        )),
    }
}

/// A note given by number or name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum NoteRef {
    /// MIDI note 0-127 (60 = C3).
    Number(u8),
    /// Name like "C3", "F#2", "Bb1".
    Name(String),
}

impl NoteRef {
    fn key(&self) -> Result<u8, McpError> {
        match self {
            Self::Number(n) if *n <= 127 => Ok(*n),
            Self::Number(n) => Err(bad(format!("note {n} is above 127"))),
            Self::Name(s) => music::parse_note(s).map_err(bad),
        }
    }
}

/// Parses hex SysEx (`"F0 00 20 32 F7"`, spaces optional) and checks the `F0 … F7` frame.
///
/// # Errors
/// Bad hex, a missing frame, data bytes above 7F, or more than 4096 bytes.
pub fn parse_sysex(hex: &str) -> Result<Vec<u8>, String> {
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

/// A send level as normalized or dB (sends top out at 0 dB); dB wins when both are given.
fn send_volume_of(volume: Option<f64>, volume_db: Option<f64>) -> Result<Option<f64>, McpError> {
    if let Some(db) = volume_db {
        if db.is_nan() {
            return Err(bad("volume_db is not a number"));
        }
        return Ok(Some(music::send_db_to_volume(db)));
    }
    match volume {
        Some(v) if !(0.0..=1.0).contains(&v) => {
            Err(bad("volume must be 0.0-1.0 (or use volume_db)"))
        }
        other => Ok(other),
    }
}

/// Volume as normalized or dB; dB wins when both are given.
fn volume_of(volume: Option<f64>, volume_db: Option<f64>) -> Result<Option<f64>, McpError> {
    if let Some(db) = volume_db {
        if db.is_nan() {
            return Err(bad("volume_db is not a number"));
        }
        return Ok(Some(music::db_to_volume(db)));
    }
    match volume {
        Some(v) if !(0.0..=1.0).contains(&v) => {
            Err(bad("volume must be 0.0-1.0 (or use volume_db)"))
        }
        other => Ok(other),
    }
}

fn unit(name: &str, v: Option<f64>) -> Result<Option<f64>, McpError> {
    match v {
        Some(x) if !(0.0..=1.0).contains(&x) => Err(bad(format!("{name} must be 0.0-1.0"))),
        other => Ok(other),
    }
}

/// Beats in a bar of a time signature like "4/4" or "6/8" (default 4).
fn beats_per_bar(signature: &str) -> f64 {
    let mut it = signature.split('/');
    let n: f64 = it.next().and_then(|n| n.trim().parse().ok()).unwrap_or(4.0);
    let d: f64 = it.next().and_then(|d| d.trim().parse().ok()).unwrap_or(4.0);
    if n > 0.0 && d > 0.0 { n * 4.0 / d } else { 4.0 }
}

/// A shaped 0-1 position for `curve`.
fn shaped(t: f64, curve: &str) -> f64 {
    match curve {
        "ease_in" => t * t,
        "ease_out" => 1.0 - (1.0 - t) * (1.0 - t),
        _ => t,
    }
}

/// The project without the detail: what is needed to decide the next call.
fn summarize_project(snap: &Json) -> Json {
    fn track(t: &Json) -> Json {
        let names = |k: &str| -> Vec<Json> {
            t[k].as_array()
                .map(|a| a.iter().map(|x| x["name"].clone()).collect())
                .unwrap_or_default()
        };
        let mut m = serde_json::Map::new();
        for k in ["index", "name", "type", "volume_text", "pan_text"] {
            m.insert(k.into(), t[k].clone());
        }
        for k in ["mute", "solo", "arm"] {
            if t[k] == true {
                m.insert(k.into(), true.into());
            }
        }
        m.insert("devices".into(), names("devices").into());
        let slots: Vec<Json> = t["clips"]
            .as_array()
            .map(|a| a.iter().map(|c| c["slot"].clone()).collect())
            .unwrap_or_default();
        m.insert("clip_slots".into(), slots.into());
        let sends: Vec<Json> = t["sends"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s["volume"].as_f64().is_some_and(|v| v > 0.0))
                    .map(|s| json!({"name": s["name"], "volume_text": s["volume_text"]}))
                    .collect()
            })
            .unwrap_or_default();
        if !sends.is_empty() {
            m.insert("sends".into(), sends.into());
        }
        Json::Object(m)
    }
    let list = |k: &str| -> Vec<Json> {
        snap[k]
            .as_array()
            .map(|a| a.iter().map(track).collect())
            .unwrap_or_default()
    };
    let mut o = snap.as_object().cloned().unwrap_or_default();
    o.insert("tracks".into(), list("tracks").into());
    o.insert("effect_tracks".into(), list("effect_tracks").into());
    if snap["master"].is_object() {
        o.insert("master".into(), track(&snap["master"]));
    }
    if let Some(sc) = snap["scenes"].as_array() {
        let sc: Vec<Json> = sc
            .iter()
            .map(|s| json!({"index": s["index"], "name": s["name"], "clip_count": s["clip_count"]}))
            .collect();
        o.insert("scenes".into(), sc.into());
    }
    o.insert("summary".into(), true.into());
    Json::Object(o)
}

// ---------------------------------------------------------------- parameters

/// What `get_project` returns.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ProjectArgs {
    /// true: every mixer value, send, device and clip in full. Default: a summary
    /// (name, type, volume, pan, mute/solo, device names, clip slots); use `get_track` for one
    /// track in full.
    pub detail: Option<bool>,
}

/// A device parameter to set as part of several.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ParamSetting {
    /// Remote-control page by name or 1-based number; default: the call's page, else current.
    pub page: Option<Ref>,
    /// Parameter name or 1-based number (1-8) on the page.
    pub param: Ref,
    /// Value 0.0-1.0.
    pub value: f64,
}

/// Several device parameters at once.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetParamsArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// Default page for settings that give none.
    pub page: Option<Ref>,
    /// The parameters to set, in order.
    pub params: Vec<ParamSetting>,
}

/// One device of a chain.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChainDevice {
    /// Bitwig device or plugin name, e.g. "EQ-5", "Compressor".
    pub name: Option<String>,
    /// Preset name instead of a bare device.
    pub preset: Option<String>,
    /// Parameters to set after loading (by name or number, optionally per page).
    #[serde(default)]
    pub params: Vec<ParamSetting>,
}

/// A chain of devices on a track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertChainArgs {
    /// Track name or 1-based number; "master"; "fx N".
    pub track: Ref,
    /// The devices, loaded in this order at the end of the chain.
    pub devices: Vec<ChainDevice>,
}

/// A send from one track to a new effect track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReturnSend {
    /// Source track name or 1-based number.
    pub track: Ref,
    /// Send level in dB.
    pub volume_db: f64,
}

/// A new effect (return) track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateReturnArgs {
    /// Name of the return, e.g. "Reverb"; sends use it.
    pub name: String,
    /// Devices of the chain, in order, e.g. [{"name": "Reverb", "params": [...]}].
    #[serde(default)]
    pub devices: Vec<ChainDevice>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// Tracks to send into it, with levels.
    #[serde(default)]
    pub sends: Vec<ReturnSend>,
}

/// The master chain.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct MasterChainArgs {
    /// Devices instead of the default [EQ-5, Compressor, Peak Limiter].
    pub devices: Option<Vec<ChainDevice>>,
}

/// A track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackArgs {
    /// Track name or 1-based number; "master"; "fx N" for effect tracks.
    pub track: Ref,
}

/// Transport action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransportArgs {
    /// What to do.
    pub action: TransportAction,
}

/// Transport settings; only given fields change.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetTransportArgs {
    /// Tempo in BPM (20-666).
    pub tempo: Option<f64>,
    /// Arranger loop on/off.
    pub r#loop: Option<bool>,
    /// Loop start in beats.
    pub loop_start: Option<f64>,
    /// Loop length in beats.
    pub loop_length: Option<f64>,
    /// Metronome on/off.
    pub metronome: Option<bool>,
    /// Play position in beats.
    pub position: Option<f64>,
    /// Like "4/4", "6/8".
    pub time_signature: Option<String>,
    /// Punch in.
    pub punch_in: Option<bool>,
    /// Punch out.
    pub punch_out: Option<bool>,
    /// Arranger overdub.
    pub arranger_overdub: Option<bool>,
    /// Launcher overdub.
    pub launcher_overdub: Option<bool>,
    /// Arranger automation write.
    pub automation_write: Option<bool>,
    /// "latch", "touch" or "write".
    pub automation_mode: Option<String>,
    /// "none", "one_bar", "two_bars" or "four_bars".
    pub pre_roll: Option<String>,
    /// Default launch quantization: "none", "8", "4", "2", "1", "1/2", "1/4", "1/8", "1/16".
    pub launch_quantization: Option<String>,
}

/// One send level.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendArg {
    /// Send by effect-track name or 1-based number.
    pub send: Ref,
    /// Level 0.0-1.0.
    pub volume: Option<f64>,
    /// Level in dB (wins over volume).
    pub volume_db: Option<f64>,
}

/// Track settings; only given fields change.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetTrackArgs {
    /// Track name or 1-based number; "master"; "fx N".
    pub track: Ref,
    /// New name.
    pub name: Option<String>,
    /// "#rrggbb" or a name (red, orange, yellow, green, teal, blue, purple, pink, ...).
    pub color: Option<String>,
    /// Volume 0.0-1.0 (0.79 ≈ 0 dB).
    pub volume: Option<f64>,
    /// Volume in dB, e.g. -6 (wins over volume).
    pub volume_db: Option<f64>,
    /// Pan 0.0 (left) - 1.0 (right), 0.5 = center.
    pub pan: Option<f64>,
    /// Mute.
    pub mute: Option<bool>,
    /// Solo.
    pub solo: Option<bool>,
    /// Record arm.
    pub arm: Option<bool>,
    /// Activated (deactivated tracks use no CPU).
    pub activated: Option<bool>,
    /// Input monitoring: "on", "off" or "auto".
    pub monitor: Option<String>,
    /// Send levels.
    #[serde(default)]
    pub sends: Vec<SendArg>,
}

/// A new track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTrackArgs {
    /// audio, instrument or effect.
    pub kind: TrackKind,
    /// Name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// 1-based position among main tracks; default: at the end.
    pub position: Option<u32>,
    /// Device to load by name, e.g. "Polymer", "Drum Machine", "HW Instrument".
    pub device: Option<String>,
    /// Preset to load by name instead of a bare device.
    pub preset: Option<String>,
}

/// Track action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackActionArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// select, duplicate, delete, stop_clips, return_to_arrangement or show.
    pub action: TrackAction,
}

/// A new scene.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSceneArgs {
    /// Name, e.g. "Verse".
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
}

/// Scene settings.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetSceneArgs {
    /// Scene name or 1-based number.
    pub scene: Ref,
    /// New name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
}

/// Scene action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SceneActionArgs {
    /// Scene name or 1-based number.
    pub scene: Ref,
    /// launch, select, duplicate or delete.
    pub action: SceneAction,
}

/// One note.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NoteArg {
    /// 60 or "C3".
    pub note: NoteRef,
    /// Start in beats from the clip start.
    pub start: f64,
    /// Length in beats.
    pub length: f64,
    /// Velocity 1-127 (default 100).
    pub velocity: Option<u8>,
}

/// A new clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateClipArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Length in whole beats (4 = one bar of 4/4).
    pub length_beats: u32,
    /// Clip name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// Notes to write (exact positions, no live recording).
    #[serde(default)]
    pub notes: Vec<NoteArg>,
}

/// Notes for an existing clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteNotesArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// The notes.
    pub notes: Vec<NoteArg>,
    /// Replace the clip's notes (default) or add to them.
    pub replace: Option<bool>,
}

/// A clip to read.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadNotesArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Grid as a note value: "1/16" (default, a sixteenth note), "1/32", "1/8T"; starts are
    /// reported on this grid.
    pub resolution: Option<String>,
}

/// Clip action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipActionArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// launch, stop, select, record, delete, duplicate or show.
    pub action: ClipAction,
}

/// Clip settings.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetClipArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// New name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// Loop length in beats.
    pub loop_length_beats: Option<f64>,
    /// Loop on/off.
    pub r#loop: Option<bool>,
    /// Transpose all notes by semitones.
    pub transpose: Option<i32>,
    /// Quantize note starts, amount 0.0-1.0.
    pub quantize: Option<f64>,
}

/// Where a device goes.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// End of the chain.
    #[default]
    End,
    /// Start of the chain.
    Start,
    /// After `relative_to`.
    After,
    /// Before `relative_to`.
    Before,
    /// Replacing `relative_to`.
    Replace,
}

/// A device to insert.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertDeviceArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Bitwig device or plugin name as shown in the browser, e.g. "Polymer", "Compressor+".
    pub name: Option<String>,
    /// Preset name to load instead (searched in the Presets tab).
    pub preset: Option<String>,
    /// CLAP plugin id, e.g. "org.surge-synth-team.surge-xt".
    pub clap_id: Option<String>,
    /// VST3 plugin id.
    pub vst3_id: Option<String>,
    /// Absolute path of a preset/sample file.
    pub file: Option<String>,
    /// end (default), start, after, before or replace.
    pub at: Option<Placement>,
    /// Device (name or 1-based number) for after/before/replace.
    pub relative_to: Option<Ref>,
}

/// A device.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeviceArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position in the chain.
    pub device: Ref,
    /// Remote-control page by name or 1-based number; default: current page.
    pub page: Option<Ref>,
}

/// A device parameter.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetParamArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// Remote-control page by name or 1-based number; default: current page.
    pub page: Option<Ref>,
    /// Parameter name or 1-based number (1-8) on the page.
    pub param: Ref,
    /// Value 0.0-1.0.
    pub value: f64,
}

/// Device action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeviceActionArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// enable, disable, toggle, delete, duplicate, open_window, close_window, next_preset,
    /// previous_preset or select.
    pub action: DeviceAction,
}

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

/// Action search.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListActionsArgs {
    /// Words to search in action ids and names, e.g. "save", "export", "group".
    pub query: String,
}

/// Action to run.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InvokeActionArgs {
    /// Action id from list_actions.
    pub id: String,
}

/// Undo or redo.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAction {
    /// Undo.
    Undo,
    /// Redo.
    Redo,
}

/// History.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HistoryArgs {
    /// undo or redo.
    pub action: HistoryAction,
    /// How many steps (default 1, max 50).
    pub steps: Option<u32>,
}

/// Window changes.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetViewArgs {
    /// "arrange", "mix" or "edit".
    pub layout: Option<String>,
    /// Panel to toggle: inspector, devices, mixer, note_editor, automation_editor, browser,
    /// fullscreen.
    pub toggle: Option<String>,
}

/// Prompt: song sketch.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SketchArgs {
    /// Genre or mood, e.g. "deep house", "lo-fi hip hop".
    pub genre: String,
    /// Tempo in BPM.
    pub tempo: Option<String>,
    /// Key, e.g. "A minor".
    pub key: Option<String>,
}

/// Prompt: hardware setup.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HardwarePromptArgs {
    /// Synth model, e.g. "Korg Minilogue XD".
    pub synth: String,
}

// ---------------------------------------------------------------- composing parameters

/// Replace a clip's notes or add to them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Clear the clip first.
    #[default]
    Replace,
    /// Keep existing notes.
    Add,
}

/// Where generated notes go.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Target {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name. An empty slot gets a new clip.
    pub slot: Ref,
    /// Clip name.
    pub name: Option<String>,
    /// replace (default) or add.
    pub mode: Option<WriteMode>,
}

/// Chords.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteChordsArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key, e.g. "A minor", "F# dorian"; needed for Roman numerals.
    pub key: Option<String>,
    /// Chord symbols ("Am7 Fmaj7 C G") or Roman numerals in the key ("i VI III VII").
    pub progression: String,
    /// Beats each chord lasts (default 4 = one 4/4 bar).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave of the chord roots (default 3; C3 = 60).
    pub octave: Option<i32>,
    /// close (smooth voice leading, default), open, drop2 or root.
    pub voicing: Option<theory::Voicing>,
    /// Rhythm per chord as steps, e.g. "x..x..x." (x hit, X accent, . rest, _ tie); default:
    /// held for the whole chord.
    pub rhythm: Option<String>,
    /// Step length of the rhythm: "1/16" (default), "1/8", "1/4", or beats.
    pub step: Option<String>,
    /// Velocity 1-127 (default 90).
    pub velocity: Option<u8>,
}

/// Bassline.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteBasslineArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key, e.g. "A minor"; needed for Roman numerals.
    pub key: Option<String>,
    /// Chords to follow, as symbols or Roman numerals.
    pub progression: String,
    /// Beats each chord lasts (default 4).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave (default 1; C1 = 36).
    pub octave: Option<i32>,
    /// root (default), octave, fifth, arpeggio or walking.
    pub style: Option<theory::BassStyle>,
    /// Hits per chord as steps (default "x.xx" in eighths).
    pub rhythm: Option<String>,
    /// Step length: "1/8" (default), "1/16", ...
    pub step: Option<String>,
    /// Velocity (default 100).
    pub velocity: Option<u8>,
}

/// Arpeggio.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteArpArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key; needed for Roman numerals.
    pub key: Option<String>,
    /// Chords to arpeggiate.
    pub progression: String,
    /// Beats each chord lasts (default 4).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave (default 3).
    pub octave: Option<i32>,
    /// up (default), down, up_down or random.
    pub direction: Option<theory::ArpMode>,
    /// Note length: "1/16" (default), "1/8", "1/8T"...
    pub rate: Option<String>,
    /// Octaves spanned (default 1, max 4).
    pub octaves: Option<u8>,
    /// Gate 0.05-1 of the rate (default 0.8).
    pub gate: Option<f64>,
    /// Velocity (default 90).
    pub velocity: Option<u8>,
    /// Seed for random (default 1).
    pub seed: Option<u64>,
}

/// Melody.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteMelodyArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key, e.g. "A minor"; the melody stays in it.
    pub key: String,
    /// The chords underneath: symbols or Roman numerals in the key. Notes on a chord change
    /// are chord tones.
    pub progression: String,
    /// Beats each chord lasts (default 4).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave of the lowest note (default 4).
    pub octave: Option<i32>,
    /// Octaves the melody spans, 1-3 (default 1).
    pub span: Option<u8>,
    /// arch (default), rise, fall, wave or flat: the shape of the motif.
    pub contour: Option<theory::Contour>,
    /// How busy it is, 0-1 (default 0.6).
    pub density: Option<f64>,
    /// Smallest step: "1/8" (default), "1/16", "1/8T"...
    pub rate: Option<String>,
    /// Length of the motif in beats (default 4); it repeats through the clip.
    pub motif_beats: Option<f64>,
    /// Rhythm of the motif as steps ("x.x.xx.." ; x hit, . rest), instead of density.
    pub rhythm: Option<String>,
    /// How much each repetition changes, 0-1 (default 0.3; 0 = exact repeats).
    pub variation: Option<f64>,
    /// Velocity (default 95).
    pub velocity: Option<u8>,
    /// Seed (default 1); another seed is another hook.
    pub seed: Option<u64>,
}

/// One Euclidean voice.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EuclidArg {
    /// Hits.
    pub hits: usize,
    /// Steps.
    pub steps: usize,
    /// Rotation (default 0).
    pub rotate: Option<usize>,
}

/// Drums.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteDrumsArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Instrument profile whose named keys to use (default "generic": General MIDI drums,
    /// which Bitwig's Drum Machine follows: kick, snare, clap, closed_hat, open_hat, ...).
    pub instrument: Option<String>,
    /// Voice → steps, e.g. {"kick": "x...x...x...x...", "closed_hat": "..x...x...x...x."}.
    /// Voices are key names from the profile, note names ("C1") or numbers.
    #[serde(default)]
    pub patterns: std::collections::BTreeMap<String, String>,
    /// Voice → Euclidean rhythm, e.g. {"rim": {"hits": 3, "steps": 8}}.
    #[serde(default)]
    pub euclid: std::collections::BTreeMap<String, EuclidArg>,
    /// Step length: "1/16" (default), "1/8"...
    pub step: Option<String>,
    /// Clip length in beats (default 4).
    pub length_beats: Option<f64>,
    /// Velocity (default 100; X accents get +25).
    pub velocity: Option<u8>,
}

/// Transform a clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransformClipArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Operations in order, e.g. [{"op": "transpose", "semitones": 7},
    /// {"op": "humanize", "timing": 0.02, "velocity": 8}].
    pub ops: Vec<theory::Transform>,
}

/// One place a clip is copied to.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CopyDest {
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Variation for this copy only, e.g. [{"op": "transpose", "semitones": 7}] or
    /// [{"op": "thin", ...}]; same operations as transform_clip.
    pub ops: Option<Vec<theory::Transform>>,
    /// Name of the copy; default: unchanged.
    pub name: Option<String>,
}

/// Copy a clip into other slots.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CopyClipArgs {
    /// Source track name or 1-based number.
    pub track: Ref,
    /// Source slot: 1-based number or scene name.
    pub slot: Ref,
    /// Target track; default: the source track.
    pub to_track: Option<Ref>,
    /// The slots to copy to, each with an optional variation. Existing clips there are replaced.
    pub to: Vec<CopyDest>,
}

/// Record into a slot.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordClipArgs {
    /// Track to record on: an audio track whose input is the synth (or a mic), or an
    /// instrument track for MIDI.
    pub track: Ref,
    /// Slot: 1-based number or scene name (should be empty).
    pub slot: Ref,
    /// Length in bars (1-64).
    pub bars: u32,
    /// After recording: play (default) or stop.
    pub then: Option<String>,
    /// Notes to play into it while recording (e.g. to capture a hardware synth): same format
    /// as create_clip. Needs a HW Instrument on `play_on`.
    #[serde(default)]
    pub notes: Vec<NoteArg>,
    /// Track that plays `notes` (e.g. the HW Instrument track of the synth being recorded).
    pub play_on: Option<Ref>,
}

/// The song plan.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SongPlanArgs {
    /// The plan as JSON: anything useful, e.g. {"title", "genre", "key", "tempo",
    /// "sections": [{"name", "bars", "scene", "energy"}], "tracks": {"Bass": "role"},
    /// "notes": [...]}. Replaces the stored plan.
    pub plan: Json,
}

/// Build the parts of a song.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct BuildSongArgs {
    /// Sections to build: [{"name": "Verse", "scene": "Verse", "parts": [{"kind": "chords",
    /// "track": "Pad", "progression": "i VI III VII"}, ...]}]. A part is a write_drums,
    /// write_bassline, write_chords, write_arp, write_melody, create_clip or copy_clip call:
    /// `kind` is drums, bass, chords, arp, melody, clip or copy, the other fields are that tool's arguments. `slot`
    /// defaults to the section's `scene` (else its `name`), `key` to the song plan's key.
    /// Default: the `sections` of the stored song plan.
    pub sections: Option<Vec<Json>>,
    /// Build only the sections with these names.
    pub only: Option<Vec<String>>,
}

/// Prompt: compose.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ComposeArgs {
    /// Idea, genre or mood, e.g. "dark techno at 130", "dreamy synthwave".
    pub idea: String,
    /// Hardware synths to use, e.g. "Model D for bass, 2600 for leads".
    pub hardware: Option<String>,
}

// ---------------------------------------------------------------- export parameters

/// A clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipRef {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
}

/// MIDI export.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportMidiArgs {
    /// Export every clip of this scene, one MIDI track per Bitwig track.
    pub scene: Option<Ref>,
    /// Or these clips.
    #[serde(default)]
    pub clips: Vec<ClipRef>,
    /// File name (without folder) or absolute path; default: PROJECT-TIME.mid in
    /// the music folder's bitwig-mcp-rs/.
    pub path: Option<String>,
}

/// Audio capture.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CaptureArgs {
    /// Bars to record at the project tempo (default 8; max 512).
    pub bars: Option<u32>,
    /// Launch this scene from bar 1 (launcher songs); default: play the arranger from the
    /// start.
    pub scene: Option<Ref>,
    /// wav (default, 32-bit float), flac or mp3.
    pub format: Option<capture::Format>,
    /// Also record each track alone (stems), one pass per track: takes bars x tracks of time.
    pub stems: Option<bool>,
    /// Seconds recorded after the last bar for reverb and delay tails (default 2).
    pub tail_seconds: Option<f64>,
    /// File name (without folder) or absolute path, without extension.
    pub path: Option<String>,
}

/// Bitwig's own export dialogs.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    /// Export Audio (offline render, stems, formats).
    Audio,
    /// Export MIDI.
    Midi,
    /// Export DAWproject (for other DAWs).
    Dawproject,
}

/// Which dialog.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportDialogArgs {
    /// audio, midi or dawproject.
    pub kind: ExportKind,
}

// ---------------------------------------------------------------- sound parameters

/// A drum kit.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DrumKitArgs {
    /// Track with the Drum Machine.
    pub track: Ref,
    /// The Drum Machine device (name or 1-based position; default "Drum Machine").
    pub device: Option<Ref>,
    /// Pad → device to load, e.g. {"kick": "v8 Kick", "snare": "v8 Snare", "clap": "v8 Clap",
    /// "closed_hat": "v8 Hat", "open_hat": "v8 Hat"}. Pads are General MIDI key names, note
    /// names ("C1") or numbers.
    #[serde(default)]
    pub pads: std::collections::BTreeMap<String, String>,
    /// Effects added after a pad's instrument, in order, e.g. {"snare": ["Gate", "Reverb"]} for
    /// the gated 80s snare. Works on pads that already have an instrument (also from an earlier
    /// call).
    #[serde(default)]
    pub effects: std::collections::BTreeMap<String, Vec<String>>,
}

/// A scene of the arrangement to record.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ArrangementScene {
    /// Scene name or 1-based number.
    pub scene: Ref,
    /// How many bars it plays (default 8).
    pub bars: Option<u32>,
}

/// Record the scenes into the arranger.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct RecordArrangementArgs {
    /// The scenes in order. Default: the sections of the song plan (`scene` or `name`, `bars`);
    /// without a plan, every scene that holds clips, 8 bars each.
    pub scenes: Option<Vec<ArrangementScene>>,
    /// Arranger position in beats where the recording starts (default 0).
    pub start_beat: Option<f64>,
}

/// Record a parameter move as arranger automation.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordAutomationArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// Remote-control page; default: current page.
    pub page: Option<Ref>,
    /// Parameter name or 1-based number on the page.
    pub param: Ref,
    /// Start value 0-1.
    pub from: f64,
    /// End value 0-1.
    pub to: f64,
    /// Length in bars (1-64).
    pub bars: u32,
    /// Arranger position in beats where it starts (default 0).
    pub start_beat: Option<f64>,
    /// Scene to launch at the start so the sweep is heard over the music (optional).
    pub scene: Option<Ref>,
    /// linear (default), ease_in (slow start, a riser) or ease_out.
    pub curve: Option<String>,
}

/// Levels.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct MetersArgs {
    /// Measure for this many milliseconds (default 0 = the peaks since the last read). Start
    /// playback first, e.g. launch a scene, and measure a bar or two.
    pub window_ms: Option<u32>,
}

/// Device listing.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListDevicesArgs {
    /// Only names containing this (case-insensitive); also lists matching presets, e.g.
    /// "pad", "bass", "80s".
    pub query: Option<String>,
    /// Browser tab: "Devices" (default) or "Presets".
    pub content_type: Option<String>,
    /// At most this many (default 2000).
    pub max: Option<u32>,
}

// ---------------------------------------------------------------- the server

/// The bridge-backed MCP server.
#[derive(Debug, Clone)]
pub struct BridgeServer {
    bridge: Arc<Bridge>,
    profiles_dir: PathBuf,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
    /// Bitwig has one selection, one browser and one cursor clip: tools run one at a time,
    /// even when a client calls several in parallel.
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl BridgeServer {
    /// Wraps a bridge client; instrument profiles are read from `profiles_dir` on use.
    #[must_use]
    pub fn new(bridge: Arc<Bridge>, profiles_dir: PathBuf) -> Self {
        Self {
            bridge,
            profiles_dir,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
            turn: Arc::default(),
        }
    }

    async fn call(&self, c: Command) -> Result<Json, McpError> {
        self.bridge.call(c).await.map_err(err)
    }

    async fn call_long(&self, c: Command) -> Result<Json, McpError> {
        self.bridge
            .call_with(c, Duration::from_secs(45))
            .await
            .map_err(err)
    }

    /// Loads devices at the end of a track's chain and sets their parameters; a failing device
    /// is reported, the rest load.
    async fn load_chain(&self, track: &Ref, devices: &[ChainDevice]) -> Result<Json, McpError> {
        let mut report = Vec::new();
        for d in devices {
            let label = d
                .name
                .clone()
                .or_else(|| d.preset.clone())
                .unwrap_or_default();
            let inserted = self
                .insert_device(Parameters(InsertDeviceArgs {
                    track: track.clone(),
                    name: d.name.clone(),
                    preset: d.preset.clone(),
                    clap_id: None,
                    vst3_id: None,
                    file: None,
                    at: None,
                    relative_to: None,
                }))
                .await;
            if let Err(e) = inserted {
                report.push(json!({"device": label, "error": e.message}));
                continue;
            }
            let mut set = Vec::new();
            if !d.params.is_empty() {
                // the new device is the last of the chain
                let view = self.view().await?;
                let addr = view.track(track).map_err(bad)?;
                let n = view.track_view(addr).map_or(0, |t| t.devices.len() as u32);
                if n == 0 {
                    report
                        .push(json!({"device": label, "error": "device not found after loading"}));
                    continue;
                }
                for p in &d.params {
                    let r = self
                        .set_param(addr, n - 1, p.page.as_ref(), &p.param, p.value)
                        .await;
                    set.push(match r {
                        Ok(v) => v,
                        Err(e) => json!({"param": p.param, "error": e.message}),
                    });
                }
            }
            report.push(json!({"device": label, "ok": true, "params": set}));
        }
        Ok(json!({"devices": report}))
    }

    /// Runs one `build_song` part: the tool named by `kind`, with `args` as its arguments.
    async fn build_part(&self, kind: &str, args: Json) -> ToolResult {
        fn parse<T: serde::de::DeserializeOwned>(kind: &str, v: Json) -> Result<T, McpError> {
            serde_json::from_value(v).map_err(|e| bad(format!("{kind} part: {e}")))
        }
        match kind {
            "drums" => self.write_drums(Parameters(parse(kind, args)?)).await,
            "bass" => self.write_bassline(Parameters(parse(kind, args)?)).await,
            "chords" => self.write_chords(Parameters(parse(kind, args)?)).await,
            "arp" => self.write_arp(Parameters(parse(kind, args)?)).await,
            "melody" => self.write_melody(Parameters(parse(kind, args)?)).await,
            "clip" => self.create_clip(Parameters(parse(kind, args)?)).await,
            "copy" => self.copy_clip(Parameters(parse(kind, args)?)).await,
            other => Err(bad(format!(
                "part kind {other:?} must be drums, bass, chords, arp, melody, clip or copy"
            ))),
        }
    }

    /// Sets one parameter of a device found by `track_device`.
    async fn set_param(
        &self,
        track: TrackAddr,
        device: u32,
        page: Option<&Ref>,
        param: &Ref,
        value: f64,
    ) -> Result<Json, McpError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(bad("value must be 0.0-1.0"));
        }
        let page = self.page_index(track, device, page).await?;
        let param = match param {
            Ref::Number(0) => return Err(bad("param numbers start at 1")),
            Ref::Number(n) => n - 1,
            Ref::Name(name) => {
                let d = self
                    .call(Command::GetDevice {
                        track,
                        device,
                        page,
                    })
                    .await?;
                let names: Vec<String> = d["params"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|p| p["name"].as_str().unwrap_or_default().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                // the page can have gaps (Test Tone: 1, 2, 8): use the parameter's own index
                let at = pick(&names, name, "parameter")? as usize;
                d["params"][at]["index"]
                    .as_u64()
                    .and_then(|i| u32::try_from(i).ok())
                    .ok_or_else(|| err("parameter without an index"))?
            }
        };
        self.call(Command::SetDeviceParam {
            track,
            device,
            page,
            param,
            value,
        })
        .await
    }

    async fn project(&self) -> Result<(Json, ProjectView), McpError> {
        let snap = self.call(Command::GetProject).await?;
        let view = ProjectView::from_snapshot(&snap).map_err(err)?;
        Ok((snap, view))
    }

    async fn view(&self) -> Result<ProjectView, McpError> {
        Ok(self.project().await?.1)
    }

    /// Resolves against a fresh snapshot; when that fails, waits a moment and tries once more.
    /// Bitwig reports a change (a rename, a new clip) a little after the call that made it, so
    /// the very next call can see the old state (seen live: rename then insert_device).
    async fn resolve<T>(
        &self,
        f: impl Fn(&ProjectView) -> Result<T, String>,
    ) -> Result<(ProjectView, T), McpError> {
        let mut attempt = 0;
        loop {
            let v = self.view().await?;
            match f(&v) {
                Ok(t) => return Ok((v, t)),
                Err(e) if attempt >= SETTLE_ATTEMPTS => return Err(bad(e)),
                Err(_) => {
                    attempt += 1;
                    tokio::time::sleep(SETTLE_RETRY).await;
                }
            }
        }
    }

    /// A track and one of its devices, resolved together (with the same retries).
    async fn track_device(&self, track: &Ref, device: &Ref) -> Result<(TrackAddr, u32), McpError> {
        let (_, pair) = self
            .resolve(|v| {
                let t = v.track(track)?;
                Ok((t, v.device(t, device)?))
            })
            .await?;
        Ok(pair)
    }

    async fn track_of(&self, r: &Ref) -> Result<(ProjectView, TrackAddr), McpError> {
        self.resolve(|v| v.track(r)).await
    }

    fn profiles(&self) -> Registry {
        let mut r = Registry::with_builtin();
        r.load_dir(&self.profiles_dir);
        r
    }

    fn profile<'a>(reg: &'a Registry, name: &str) -> Result<&'a Profile, McpError> {
        reg.get(name).ok_or_else(|| {
            bad(format!(
                "no instrument profile {name:?}; have {:?}",
                reg.names()
            ))
        })
    }

    /// Sends profile messages in order: channel messages into the track, SysEx through the
    /// extension's MIDI output.
    async fn send_messages(
        &self,
        track: TrackAddr,
        messages: &[MidiMessage],
    ) -> Result<(), McpError> {
        let mut batch: Vec<[u8; 3]> = Vec::new();
        for m in messages {
            match m {
                MidiMessage::Sysex(bytes) => {
                    if !batch.is_empty() {
                        let messages = std::mem::take(&mut batch);
                        self.call(Command::SendMidi { track, messages }).await?;
                    }
                    self.call(Command::SendSysex {
                        bytes: bytes.clone(),
                    })
                    .await?;
                }
                other => batch.extend(other.as_channel()),
            }
        }
        if !batch.is_empty() {
            self.call(Command::SendMidi {
                track,
                messages: batch,
            })
            .await?;
        }
        Ok(())
    }

    fn grid_notes(notes: &[NoteArg]) -> Result<(f64, Vec<GridNote>, bool), McpError> {
        let starts: Vec<f64> = notes.iter().map(|n| n.start).collect();
        let grid = music::choose_grid(&starts).map_err(bad)?;
        let mut out = Vec::with_capacity(notes.len());
        for (n, x) in notes.iter().zip(&grid.xs) {
            let velocity = n.velocity.unwrap_or(100);
            if velocity == 0 || velocity > 127 {
                return Err(bad("velocity must be 1-127"));
            }
            if !(n.length.is_finite() && n.length > 0.0) {
                return Err(bad("note length must be > 0 beats"));
            }
            out.push(GridNote {
                x: *x,
                key: n.note.key()?,
                velocity,
                duration: n.length,
            });
        }
        Ok((grid.step, out, grid.rounded))
    }

    async fn set_track_change(
        &self,
        track: TrackAddr,
        change: TrackChange,
    ) -> Result<Json, McpError> {
        self.call(Command::SetTrack { track, change }).await
    }

    async fn insert(
        &self,
        track: TrackAddr,
        at: Insert,
        source: DeviceSource,
    ) -> Result<Json, McpError> {
        let wanted = match &source {
            DeviceSource::Browser { name, .. } => Some(name.clone()),
            _ => None,
        };
        self.call_long(Command::InsertDevice { track, at, source })
            .await
            .map_err(|mut e| {
                let instrument = wanted
                    .as_deref()
                    .and_then(crate::devices::find)
                    .is_some_and(|g| matches!(g.kind, "instrument" | "drum"));
                if instrument && e.message.contains("no browser result") {
                    e.message = format!(
                        "{} (this position only takes effects: the track already has an \
                         instrument there. Use at=replace with relative_to that instrument, or \
                         a new track)",
                        e.message
                    )
                    .into();
                }
                e
            })
    }

    async fn page_index(
        &self,
        track: TrackAddr,
        device: u32,
        page: Option<&Ref>,
    ) -> Result<Option<u32>, McpError> {
        match page {
            None => Ok(None),
            Some(Ref::Number(0)) => Err(bad("page numbers start at 1")),
            Some(Ref::Number(n)) => Ok(Some(n - 1)),
            Some(Ref::Name(name)) => {
                let d = self
                    .call(Command::GetDevice {
                        track,
                        device,
                        page: None,
                    })
                    .await?;
                let pages: Vec<String> = d["pages"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| p.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                pick(&pages, name, "page").map(Some)
            }
        }
    }
}

/// Refuses to touch a "new" track whose type does not fit the kind asked for: renaming the
/// wrong track (once: the master) is worse than failing.
fn check_created_type(kind: TrackKind, ty: Option<&str>) -> Result<(), String> {
    let ok = match (kind, ty) {
        (_, None) => true, // older extension: no type reported
        (TrackKind::Instrument, Some(t)) => t == "Instrument" || t == "Hybrid",
        (TrackKind::Audio, Some(t)) => t == "Audio" || t == "Hybrid",
        (TrackKind::Effect, Some(t)) => t == "Effect",
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "Bitwig reported the new track as {:?}, not {kind:?}; nothing was changed. \
             Check get_project.",
            ty.unwrap_or_default()
        ))
    }
}

/// Index of `name` among `names` (exact, then unique substring), 0-based.
fn pick(names: &[String], name: &str, what: &str) -> Result<u32, McpError> {
    let n = name.trim().to_lowercase();
    if let Ok(num) = n.parse::<u32>() {
        return num
            .checked_sub(1)
            .filter(|i| (*i as usize) < names.len())
            .ok_or_else(|| bad(format!("{what} {num} does not exist")));
    }
    let idx = names
        .iter()
        .position(|p| p.to_lowercase() == n)
        .or_else(|| {
            let hits: Vec<usize> = names
                .iter()
                .enumerate()
                .filter(|(_, p)| p.to_lowercase().contains(&n))
                .map(|(i, _)| i)
                .collect();
            (hits.len() == 1).then(|| hits[0])
        })
        .ok_or_else(|| bad(format!("no unique {what} {name:?}; have {names:?}")))?;
    u32::try_from(idx).map_err(err)
}

/// The snapshot entry of a clip (Null when the slot is empty).
fn track_clip(snapshot: &Json, track: TrackAddr, slot: u32) -> Json {
    let list = match track.bank {
        super::protocol::TrackBank::Main => &snapshot["tracks"],
        super::protocol::TrackBank::Effect => &snapshot["effect_tracks"],
        super::protocol::TrackBank::Master => return Json::Null,
    };
    list.as_array()
        .and_then(|l| l.iter().find(|t| t["index"] == track.index))
        .and_then(|t| t["clips"].as_array())
        .and_then(|c| c.iter().find(|c| c["slot"] == slot))
        .cloned()
        .unwrap_or(Json::Null)
}

/// A step or rate like "1/16", "1/8T", "0.5" in beats.
fn parse_step(text: Option<&str>, default: f64) -> Result<f64, McpError> {
    let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(default);
    };
    let (body, triplet) = match t.strip_suffix(['T', 't']) {
        Some(b) => (b, true),
        None => (t, false),
    };
    let beats = match body.split_once('/') {
        // "1/16" of a whole note = 4/16 beats
        Some((n, d)) => {
            let n: f64 = n
                .trim()
                .parse()
                .map_err(|_| bad(format!("bad step {t:?}")))?;
            let d: f64 = d
                .trim()
                .parse()
                .map_err(|_| bad(format!("bad step {t:?}")))?;
            4.0 * n / d
        }
        None => body.parse().map_err(|_| bad(format!("bad step {t:?}")))?,
    };
    let beats = if triplet { beats * 2.0 / 3.0 } else { beats };
    if !(beats.is_finite() && (1.0 / 64.0..=16.0).contains(&beats)) {
        return Err(bad(format!("step {t:?} must be between 1/256 and 4 bars")));
    }
    Ok(beats)
}

fn chords_of(key: Option<&str>, progression: &str) -> Result<Vec<theory::Chord>, McpError> {
    let scale = key.map(theory::Scale::parse).transpose().map_err(bad)?;
    let chords = theory::progression(progression, scale.as_ref()).map_err(bad)?;
    if chords.len() > 64 {
        return Err(bad("at most 64 chords"));
    }
    Ok(chords)
}

fn beats_of(per_chord: Option<f64>) -> Result<f64, McpError> {
    let b = per_chord.unwrap_or(4.0);
    if !(b.is_finite() && (0.25..=64.0).contains(&b)) {
        return Err(bad("beats_per_chord must be 0.25-64"));
    }
    Ok(b)
}

fn rhythm_of(text: Option<&str>) -> Result<Option<Vec<theory::Step>>, McpError> {
    text.map(theory::steps).transpose().map_err(bad)
}

/// Resolves a drum voice: a key name of the profile, a note name or a number.
fn voice_key(profile: Option<&Profile>, voice: &str) -> Result<u8, McpError> {
    if let Some(k) = profile.and_then(|p| p.key(voice)) {
        return Ok(k);
    }
    if let Ok(n) = voice.trim().parse::<u8>()
        && n <= 127
    {
        return Ok(n);
    }
    music::parse_note(voice).map_err(|_| {
        let names: Vec<&String> = profile.map(|p| p.keys.keys().collect()).unwrap_or_default();
        bad(format!(
            "unknown drum voice {voice:?}: use a key name {names:?}, a note name or a number"
        ))
    })
}

/// Resolves an export path: absolute stays, a bare name goes into the default folder; the
/// extension is set to `ext`.
fn export_path(path: Option<&str>, default_stem: &str, ext: &str) -> PathBuf {
    let p = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        Some(p) => capture::default_dir().join(p),
        None => capture::default_dir().join(capture::safe_name(default_stem)),
    };
    p.with_extension(ext)
}

/// Tempo and beats per bar from a snapshot.
fn tempo_of(snapshot: &Json) -> (f64, f64, u8, u8) {
    let tempo = snapshot["transport"]["tempo"]
        .as_f64()
        .unwrap_or(120.0)
        .max(20.0);
    let (num, den) = snapshot["transport"]["time_signature"]
        .as_str()
        .and_then(|s| s.split_once('/'))
        .and_then(|(n, d)| Some((n.trim().parse::<u8>().ok()?, d.trim().parse::<u8>().ok()?)))
        .unwrap_or((4, 4));
    (
        tempo,
        f64::from(num) * 4.0 / f64::from(den.max(1)),
        num,
        den,
    )
}

impl BridgeServer {
    /// Writes generated notes into a slot: creates the clip when the slot is empty (length
    /// rounded up to whole beats), else replaces or adds notes and sets the loop length.
    async fn put_notes(
        &self,
        target: &Target,
        notes: Vec<crate::midi::Note>,
        length: f64,
    ) -> ToolResult {
        if notes.is_empty() {
            return Err(bad("nothing to write: the pattern has no notes"));
        }
        if notes.len() > 4000 {
            return Err(bad("more than 4000 notes; write fewer bars at a time"));
        }
        let (view, track) = self.track_of(&target.track).await?;
        let slot = view.slot(&target.slot).map_err(bad)?;
        let whole = length.ceil().clamp(1.0, 1024.0) as u32;
        let existed = view.has_clip(track, slot);
        if !existed {
            self.call(Command::CreateClip {
                track,
                slot,
                length: whole,
            })
            .await?;
        }
        let args: Vec<NoteArg> = notes
            .iter()
            .map(|n| NoteArg {
                note: NoteRef::Number(n.note),
                start: n.start_beats,
                length: n.length_beats,
                velocity: Some(n.velocity.clamp(1, 127)),
            })
            .collect();
        let (step, grid, rounded) = Self::grid_notes(&args)?;
        let replace = target.mode.unwrap_or_default() == WriteMode::Replace;
        let written = self
            .call(Command::WriteNotes {
                track,
                slot,
                step,
                clear: replace,
                notes: grid,
            })
            .await?;
        let change = ClipChange {
            name: target.name.clone(),
            loop_length: (existed && replace).then_some(length),
            ..ClipChange::default()
        };
        if change.name.is_some() || change.loop_length.is_some() {
            self.call(Command::SetClip {
                track,
                slot,
                change,
            })
            .await?;
        }
        out(&json!({
            "slot": slot + 1,
            "notes_written": written["notes"].as_array().map_or(0, Vec::len),
            "length_beats": length,
            "new_clip": !existed,
            "rounded_to_1_64": rounded,
        }))
    }

    async fn read_clip_notes(
        &self,
        track: TrackAddr,
        slot: u32,
    ) -> Result<(Vec<crate::midi::Note>, f64), McpError> {
        let coarse = self
            .call(Command::ReadNotes {
                track,
                slot,
                step: 0.25,
            })
            .await?;
        let length = coarse["clip"]["loop_length"].as_f64().unwrap_or(4.0);
        // finest grid that still covers the clip (4096 steps)
        let step = if length <= 64.0 { 1.0 / 64.0 } else { 0.25 };
        let r = if step == 0.25 {
            coarse
        } else {
            self.call(Command::ReadNotes { track, slot, step }).await?
        };
        let notes = r["notes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| crate::midi::Note {
                        note: n["key"].as_u64().unwrap_or(0).min(127) as u8,
                        start_beats: n["x"].as_f64().unwrap_or(0.0) * step,
                        length_beats: n["duration"].as_f64().unwrap_or(0.25),
                        velocity: n["velocity"].as_u64().unwrap_or(100).clamp(1, 127) as u8,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((notes, length))
    }
}

#[tool_router]
impl BridgeServer {
    // ------------------------------------------------------------ reading

    #[tool(
        description = "Connection status: server version, bridge address, extension and protocol version, project name."
    )]
    async fn status(&self) -> ToolResult {
        match self.call(Command::GetProject).await {
            Ok(snap) => out(&json!({
                "connected": true,
                "server_version": env!("CARGO_PKG_VERSION"),
                "bridge": self.bridge.addr(),
                "extension": self.bridge.hello(),
                "project": snap["application"]["project_name"],
            })),
            Err(e) => out(&json!({
                "connected": false,
                "server_version": env!("CARGO_PKG_VERSION"),
                "bridge": self.bridge.addr(),
                "error": e.message,
            })),
        }
    }

    #[tool(
        description = "The project: transport, scenes and every track, effect track and the master. A summary by default (volume, pan, mute/solo, device names, clip slots); detail:true for everything. Read this first."
    )]
    async fn get_project(&self, Parameters(a): Parameters<ProjectArgs>) -> ToolResult {
        let snap = self.project().await?.0;
        if a.detail.unwrap_or(false) {
            out(&snap)
        } else {
            out(&summarize_project(&snap))
        }
    }

    #[tool(
        description = "One track in full: mixer values with display text, sends, devices and clips."
    )]
    async fn get_track(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let (snap, view) = self.project().await?;
        let addr = view.track(&a.track).map_err(bad)?;
        let list = match addr.bank {
            super::protocol::TrackBank::Main => &snap["tracks"],
            super::protocol::TrackBank::Effect => &snap["effect_tracks"],
            super::protocol::TrackBank::Master => return out(&snap["master"]),
        };
        let t = list
            .as_array()
            .and_then(|l| l.iter().find(|t| t["index"] == addr.index))
            .ok_or_else(|| err("track vanished"))?;
        out(t)
    }

    // ------------------------------------------------------------ transport

    #[tool(
        description = "Transport: play, stop, toggle_play, record, restart, rewind, fast_forward, tap_tempo, stop_all_clips, add_cue_marker, next_cue_marker, previous_cue_marker."
    )]
    async fn transport(&self, Parameters(a): Parameters<TransportArgs>) -> ToolResult {
        out(&self.call(Command::Transport { action: a.action }).await?)
    }

    #[tool(
        description = "Change tempo, loop, metronome, position, time signature, punch, overdub, automation and launch quantization. Only given fields change. Replies with the transport state."
    )]
    async fn set_transport(&self, Parameters(a): Parameters<SetTransportArgs>) -> ToolResult {
        if a.tempo.is_some_and(|t| !(20.0..=666.0).contains(&t)) {
            return Err(bad("tempo must be 20-666 BPM"));
        }
        if let Some(sig) = &a.time_signature {
            let ok = sig.split_once('/').is_some_and(|(n, d)| {
                n.trim().parse::<u8>().is_ok_and(|n| n > 0)
                    && d.trim().parse::<u8>().is_ok_and(|d| d.is_power_of_two())
            });
            if !ok {
                return Err(bad("time_signature must look like 4/4 or 6/8"));
            }
        }
        let change = TransportChange {
            tempo: a.tempo,
            r#loop: a.r#loop,
            loop_start: a.loop_start,
            loop_length: a.loop_length,
            metronome: a.metronome,
            position: a.position,
            time_signature: a.time_signature.map(|s| s.replace(' ', "")),
            punch_in: a.punch_in,
            punch_out: a.punch_out,
            arranger_overdub: a.arranger_overdub,
            launcher_overdub: a.launcher_overdub,
            automation_write: a.automation_write,
            automation_mode: a.automation_mode,
            pre_roll: a.pre_roll,
            launch_quantization: a.launch_quantization,
        };
        out(&self.call(Command::SetTransport { change }).await?)
    }

    // ------------------------------------------------------------ tracks

    #[tool(
        description = "Create a track (audio, instrument or effect) and set it up in one go: name, color, position and a device or preset loaded by name (e.g. device \"Polymer\", \"Drum Machine\", \"HW Instrument\"). Replies with the new track."
    )]
    async fn create_track(&self, Parameters(a): Parameters<CreateTrackArgs>) -> ToolResult {
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        let position = match a.position {
            Some(0) => return Err(bad("position starts at 1")),
            Some(p) => i32::try_from(p - 1).map_err(err)?,
            None => -1,
        };
        let created = self
            .call(Command::CreateTrack {
                kind: a.kind,
                position,
            })
            .await?;
        check_created_type(a.kind, created["type"].as_str()).map_err(err)?;
        let track: TrackAddr = serde_json::from_value(created).map_err(err)?;
        let mut notes = Vec::new();
        let source = match (a.device, a.preset) {
            (_, Some(p)) => Some(DeviceSource::Browser {
                name: p,
                content_type: None,
            }),
            (Some(d), None) => Some(DeviceSource::Browser {
                name: d,
                content_type: None,
            }),
            (None, None) => None,
        };
        if let Some(source) = source
            && let Err(e) = self.insert(track, Insert::End, source).await
        {
            notes.push(format!("device not loaded: {}", e.message));
        }
        let state = self
            .set_track_change(
                track,
                TrackChange {
                    name: a.name,
                    color,
                    ..TrackChange::default()
                },
            )
            .await?;
        out(&json!({ "track": state, "warnings": notes }))
    }

    #[tool(
        description = "Change a track: name, color, volume (0-1 or volume_db), pan, mute, solo, arm, activated, monitor, send levels. Only given fields change. Replies with the track after the change, including Bitwig's display text (e.g. \"-6.0 dB\")."
    )]
    async fn set_track(&self, Parameters(a): Parameters<SetTrackArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let mut sends = Vec::new();
        for s in &a.sends {
            let index = view.send(track, &s.send).map_err(bad)?;
            let volume = send_volume_of(s.volume, s.volume_db)?
                .ok_or_else(|| bad("each send needs volume or volume_db"))?;
            sends.push(SendLevel { index, volume });
        }
        let monitor_mode = match a.monitor.as_deref().map(str::to_lowercase).as_deref() {
            None => None,
            Some("on") => Some("ON".to_owned()),
            Some("off") => Some("OFF".to_owned()),
            Some("auto") => Some("AUTO".to_owned()),
            Some(o) => return Err(bad(format!("monitor must be on, off or auto, not {o:?}"))),
        };
        let change = TrackChange {
            name: a.name,
            color: a
                .color
                .as_deref()
                .map(parse_color)
                .transpose()
                .map_err(bad)?,
            volume: volume_of(a.volume, a.volume_db)?,
            pan: unit("pan", a.pan)?,
            mute: a.mute,
            solo: a.solo,
            arm: a.arm,
            activated: a.activated,
            monitor_mode,
            sends,
        };
        out(&self.set_track_change(track, change).await?)
    }

    #[tool(
        description = "Track action: select, duplicate, delete, stop_clips, return_to_arrangement, show. delete is undoable with history."
    )]
    async fn track_action(&self, Parameters(a): Parameters<TrackActionArgs>) -> ToolResult {
        let (_, track) = self.track_of(&a.track).await?;
        out(&self
            .call(Command::TrackAction {
                track,
                action: a.action,
            })
            .await?)
    }

    // ------------------------------------------------------------ scenes

    #[tool(
        description = "Create a scene at the end of the scene list, with a name and color. Replies with the scene number."
    )]
    async fn create_scene(&self, Parameters(a): Parameters<CreateSceneArgs>) -> ToolResult {
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        let r = self.call(Command::CreateScene).await?;
        let index = r["index"].as_u64().ok_or_else(|| err("no scene index"))?;
        let scene = u32::try_from(index).map_err(err)?;
        if a.name.is_some() || color.is_some() {
            self.call(Command::SetScene {
                scene,
                name: a.name.clone(),
                color,
            })
            .await?;
        }
        out(&json!({"scene": scene + 1, "name": a.name}))
    }

    #[tool(description = "Rename or recolor a scene.")]
    async fn set_scene(&self, Parameters(a): Parameters<SetSceneArgs>) -> ToolResult {
        let (_, scene) = self.resolve(|v| v.scene(&a.scene)).await?;
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        out(&self
            .call(Command::SetScene {
                scene,
                name: a.name,
                color,
            })
            .await?)
    }

    #[tool(description = "Scene action: launch, select, duplicate, delete.")]
    async fn scene_action(&self, Parameters(a): Parameters<SceneActionArgs>) -> ToolResult {
        let (_, scene) = self.resolve(|v| v.scene(&a.scene)).await?;
        out(&self
            .call(Command::SceneAction {
                scene,
                action: a.action,
            })
            .await?)
    }

    // ------------------------------------------------------------ clips

    #[tool(
        description = "Create a launcher clip in a slot (slot = 1-based number or scene name) with exact notes: [{note: \"C3\" or 60, start: beats, length: beats, velocity}]. Notes are written directly, not recorded. Replies with the notes as Bitwig stored them."
    )]
    async fn create_clip(&self, Parameters(a): Parameters<CreateClipArgs>) -> ToolResult {
        if a.length_beats == 0 || a.length_beats > 1024 {
            return Err(bad("length_beats must be 1-1024"));
        }
        if let Some(late) = a
            .notes
            .iter()
            .find(|n| n.start >= f64::from(a.length_beats))
        {
            return Err(bad(format!(
                "a note starts at beat {} but the clip is {} beats long",
                late.start, a.length_beats
            )));
        }
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        let grid = Self::grid_notes(&a.notes)?;
        self.call(Command::CreateClip {
            track,
            slot,
            length: a.length_beats,
        })
        .await?;
        if a.name.is_some() || color.is_some() {
            self.call(Command::SetClip {
                track,
                slot,
                change: ClipChange {
                    name: a.name,
                    color,
                    ..ClipChange::default()
                },
            })
            .await?;
        }
        let written = if grid.1.is_empty() {
            json!({"notes": []})
        } else {
            self.call(Command::WriteNotes {
                track,
                slot,
                step: grid.0,
                clear: true,
                notes: grid.1,
            })
            .await?
        };
        out(&json!({"slot": slot + 1, "written": written, "rounded_to_1_64": grid.2}))
    }

    #[tool(
        description = "Write notes into an existing clip at exact positions; replace (default true) clears the old notes first."
    )]
    async fn write_notes(&self, Parameters(a): Parameters<WriteNotesArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let (step, notes, rounded) = Self::grid_notes(&a.notes)?;
        let r = self
            .call(Command::WriteNotes {
                track,
                slot,
                step,
                clear: a.replace.unwrap_or(true),
                notes,
            })
            .await?;
        out(&json!({"written": r, "rounded_to_1_64": rounded}))
    }

    #[tool(
        description = "Read the notes of a clip: [{note, name, start, length, velocity}] in beats, starts on the given grid (default 1/16)."
    )]
    async fn read_notes(&self, Parameters(a): Parameters<ReadNotesArgs>) -> ToolResult {
        let step = parse_step(a.resolution.as_deref(), 0.25)?;
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let r = self.call(Command::ReadNotes { track, slot, step }).await?;
        let notes: Vec<Json> = r["notes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| {
                        let key = n["key"].as_u64().unwrap_or(0).min(127) as u8;
                        let x = n["x"].as_f64().unwrap_or(0.0);
                        json!({
                            "note": key,
                            "name": music::note_name(key),
                            "start": x * step,
                            "length": n["duration"],
                            "velocity": n["velocity"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out(&json!({"clip": r["clip"], "resolution_beats": step, "notes": notes}))
    }

    #[tool(
        description = "Clip action on a slot: launch, stop, select, record, delete, duplicate, show."
    )]
    async fn clip_action(&self, Parameters(a): Parameters<ClipActionArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        out(&self
            .call(Command::ClipAction {
                track,
                slot,
                action: a.action,
            })
            .await?)
    }

    #[tool(
        description = "Change a clip: name, color, loop length, loop on/off, transpose (semitones), quantize (0-1)."
    )]
    async fn set_clip(&self, Parameters(a): Parameters<SetClipArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let change = ClipChange {
            name: a.name,
            color: a
                .color
                .as_deref()
                .map(parse_color)
                .transpose()
                .map_err(bad)?,
            loop_length: a.loop_length_beats,
            loop_enabled: a.r#loop,
            transpose: a.transpose,
            quantize: unit("quantize", a.quantize)?,
        };
        out(&self
            .call(Command::SetClip {
                track,
                slot,
                change,
            })
            .await?)
    }

    // ------------------------------------------------------------ devices

    #[tool(
        description = "Insert a device into a track's chain: a Bitwig device or plugin by name, a preset by name, a CLAP/VST3 plugin by id, or a file. Placement: end (default), start, or after/before/replace relative_to a device."
    )]
    async fn insert_device(&self, Parameters(a): Parameters<InsertDeviceArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let rel = || -> Result<u32, McpError> {
            let r = a
                .relative_to
                .as_ref()
                .ok_or_else(|| bad("after/before/replace need relative_to"))?;
            view.device(track, r).map_err(bad)
        };
        let at = match a.at.unwrap_or_default() {
            Placement::End => Insert::End,
            Placement::Start => Insert::Start,
            Placement::After => Insert::After(rel()?),
            Placement::Before => Insert::Before(rel()?),
            Placement::Replace => Insert::Replace(rel()?),
        };
        let source = match (a.name, a.preset, a.clap_id, a.vst3_id, a.file) {
            (Some(name), None, None, None, None) => DeviceSource::Browser {
                name,
                content_type: None,
            },
            (None, Some(name), None, None, None) => DeviceSource::Browser {
                name,
                content_type: None,
            },
            (None, None, Some(id), None, None) => DeviceSource::Clap { id },
            (None, None, None, Some(id), None) => DeviceSource::Vst3 { id },
            (None, None, None, None, Some(path)) => DeviceSource::File { path },
            _ => {
                return Err(bad(
                    "give exactly one of name, preset, clap_id, vst3_id, file",
                ));
            }
        };
        out(&self.insert(track, at, source).await?)
    }

    #[tool(
        description = "A device: enabled state, remote-control page names, and the 8 parameters of a page (name, value 0-1, display text)."
    )]
    async fn get_device(&self, Parameters(a): Parameters<DeviceArgs>) -> ToolResult {
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        let page = self.page_index(track, device, a.page.as_ref()).await?;
        out(&self
            .call(Command::GetDevice {
                track,
                device,
                page,
            })
            .await?)
    }

    #[tool(
        description = "Set a device parameter (by name or 1-8 on a remote-control page) to 0.0-1.0. Replies with the value Bitwig shows, e.g. \"1.20 kHz\"."
    )]
    async fn set_device_param(&self, Parameters(a): Parameters<SetParamArgs>) -> ToolResult {
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        out(&self
            .set_param(track, device, a.page.as_ref(), &a.param, a.value)
            .await?)
    }

    #[tool(
        description = "Set several device parameters in one call, by name or number, across remote-control pages (e.g. a darker sound: cutoff, resonance, release). Replies with what Bitwig shows for each."
    )]
    async fn set_device_params(&self, Parameters(a): Parameters<SetParamsArgs>) -> ToolResult {
        if a.params.is_empty() {
            return Err(bad("params is empty"));
        }
        if let Some(p) = a.params.iter().find(|p| !(0.0..=1.0).contains(&p.value)) {
            return Err(bad(format!("value {} must be 0.0-1.0", p.value)));
        }
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        let mut results = Vec::new();
        for p in &a.params {
            let page = p.page.as_ref().or(a.page.as_ref());
            results.push(
                self.set_param(track, device, page, &p.param, p.value)
                    .await?,
            );
        }
        out(&Json::Array(results))
    }

    #[tool(
        description = "Load a chain of devices onto a track in one call, in order, each with optional parameters (by name), e.g. a master chain or a vocal chain. Reports every device; a failing device is reported and the rest still load."
    )]
    async fn insert_chain(&self, Parameters(a): Parameters<InsertChainArgs>) -> ToolResult {
        if a.devices.is_empty() {
            return Err(bad("devices is empty"));
        }
        out(&self.load_chain(&a.track, &a.devices).await?)
    }

    #[tool(
        description = "Create an effect (return) track with a device chain and send levels in one call, e.g. a reverb bus: name \"Reverb\", devices [{\"name\": \"Reverb\"}], sends [{track: \"Pad\", volume_db: -12}]."
    )]
    async fn create_return(&self, Parameters(a): Parameters<CreateReturnArgs>) -> ToolResult {
        let created = self
            .create_track(Parameters(CreateTrackArgs {
                kind: TrackKind::Effect,
                name: Some(a.name.clone()),
                color: a.color,
                position: None,
                device: None,
                preset: None,
            }))
            .await?;
        let chain = if a.devices.is_empty() {
            Json::Null
        } else {
            self.load_chain(&Ref::Name(a.name.clone()), &a.devices)
                .await?
        };
        let mut sends = Vec::new();
        for s in &a.sends {
            let r = self
                .set_track(Parameters(SetTrackArgs {
                    track: s.track.clone(),
                    name: None,
                    color: None,
                    volume: None,
                    volume_db: None,
                    pan: None,
                    mute: None,
                    solo: None,
                    arm: None,
                    activated: None,
                    monitor: None,
                    sends: vec![SendArg {
                        send: Ref::Name(a.name.clone()),
                        volume: None,
                        volume_db: Some(s.volume_db),
                    }],
                }))
                .await;
            sends.push(match r {
                Ok(_) => json!({"track": s.track, "volume_db": s.volume_db, "ok": true}),
                Err(e) => json!({"track": s.track, "error": e.message}),
            });
        }
        out(&json!({
            "created": serde_json::from_str::<Json>(&created).unwrap_or(Json::Null),
            "chain": chain,
            "sends": sends,
        }))
    }

    #[tool(
        description = "Record the scenes, one after the other, into the arranger: the step from a loop sketch to a finished song. Launches each scene for its bars (from the song plan's sections, or given) with arranger recording on, then stops. Takes as long as the song; the arranger afterwards holds the clips. The scene launch quantization (usually one bar) keeps the cuts on the beat."
    )]
    async fn record_arrangement(
        &self,
        Parameters(a): Parameters<RecordArrangementArgs>,
    ) -> ToolResult {
        let (snap, view) = self.project().await?;
        let tempo = snap["transport"]["tempo"].as_f64().unwrap_or(120.0);
        let bar = beats_per_bar(
            snap["transport"]["time_signature"]
                .as_str()
                .unwrap_or("4/4"),
        );
        let secs_per_beat = 60.0 / tempo.max(20.0);
        // (scene index, name, bars)
        let mut plan: Vec<(u32, String, u32)> = Vec::new();
        match a.scenes {
            Some(list) => {
                for s in list {
                    let i = view.scene(&s.scene).map_err(bad)?;
                    let name = view
                        .scenes
                        .iter()
                        .find(|x| x.index == i)
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    plan.push((i, name, s.bars.unwrap_or(8)));
                }
            }
            None => {
                let from_plan = self.call(Command::GetSongPlan).await.ok().and_then(|r| {
                    serde_json::from_str::<Json>(r["text"].as_str().unwrap_or_default()).ok()
                });
                for sec in from_plan
                    .as_ref()
                    .and_then(|p| p["sections"].as_array())
                    .into_iter()
                    .flatten()
                {
                    let name = sec["scene"]
                        .as_str()
                        .or(sec["name"].as_str())
                        .unwrap_or_default();
                    if let Ok(i) = view.scene(&Ref::Name(name.to_owned())) {
                        plan.push((
                            i,
                            name.to_owned(),
                            sec["bars"].as_u64().map_or(8, |b| b as u32),
                        ));
                    }
                }
                if plan.is_empty() {
                    for sc in snap["scenes"].as_array().into_iter().flatten() {
                        if sc["clip_count"].as_u64().unwrap_or(0) > 0 {
                            plan.push((
                                sc["index"].as_u64().unwrap_or(0) as u32,
                                sc["name"].as_str().unwrap_or_default().to_owned(),
                                8,
                            ));
                        }
                    }
                }
            }
        }
        if plan.is_empty() {
            return Err(bad(
                "no scenes to record: create clips in scenes, or give scenes",
            ));
        }
        let total_bars: u32 = plan.iter().map(|p| p.2).sum();
        if plan.iter().any(|p| p.2 == 0) || total_bars > 512 {
            return Err(bad("bars must be 1 or more, 512 in all"));
        }
        // start clean: stop, rewind to the start position, arranger recording on
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        self.call(Command::Transport {
            action: TransportAction::StopAllClips,
        })
        .await?;
        self.call(Command::SetTransport {
            change: TransportChange {
                position: Some(a.start_beat.unwrap_or(0.0)),
                ..TransportChange::default()
            },
        })
        .await?;
        if snap["transport"]["recording"] != true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        let begin = tokio::time::Instant::now();
        let mut at_bars = 0u32;
        let mut timeline = Vec::new();
        for (i, (scene, name, bars)) in plan.iter().enumerate() {
            if i > 0 {
                // launch a little early: the launch quantization lands it on the bar line
                let when = begin
                    + Duration::from_secs_f64(f64::from(at_bars) * bar * secs_per_beat - 0.12);
                tokio::time::sleep_until(when).await;
            }
            self.call(Command::SceneAction {
                scene: *scene,
                action: SceneAction::Launch,
            })
            .await?;
            timeline.push(json!({"scene": name, "start_bar": at_bars + 1, "bars": bars}));
            at_bars += bars;
        }
        tokio::time::sleep_until(
            begin + Duration::from_secs_f64(f64::from(total_bars) * bar * secs_per_beat),
        )
        .await;
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        let after = self.call(Command::GetProject).await?;
        if after["transport"]["recording"] == true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        out(&json!({
            "recorded_bars": total_bars,
            "tempo": tempo,
            "timeline": timeline,
            "hint": "the clips are in the arranger now; switch the view with set_view layout ARRANGE, then save_project",
        }))
    }

    #[tool(
        description = "Record a parameter move (filter sweep, riser, volume swell) as arranger automation: sets the parameter from a start to an end value over some bars while the arranger records with automation write on. Optionally launches a scene so the sweep is made over the music. Takes as long as the bars; the move is written in small steps."
    )]
    async fn record_automation(
        &self,
        Parameters(a): Parameters<RecordAutomationArgs>,
    ) -> ToolResult {
        if !(1..=64).contains(&a.bars) {
            return Err(bad("bars must be 1-64"));
        }
        if ![a.from, a.to].iter().all(|v| (0.0..=1.0).contains(v)) {
            return Err(bad("from and to must be 0.0-1.0"));
        }
        let curve = a.curve.as_deref().unwrap_or("linear");
        if !matches!(curve, "linear" | "ease_in" | "ease_out") {
            return Err(bad("curve must be linear, ease_in or ease_out"));
        }
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        let snap = self.call(Command::GetProject).await?;
        let tempo = snap["transport"]["tempo"].as_f64().unwrap_or(120.0);
        let bar = beats_per_bar(
            snap["transport"]["time_signature"]
                .as_str()
                .unwrap_or("4/4"),
        );
        let secs = f64::from(a.bars) * bar * 60.0 / tempo.max(20.0);
        let steps = ((secs / 0.25).round() as u32).clamp(4, 400);
        let scene = match &a.scene {
            Some(r) => Some(self.resolve(|v| v.scene(r)).await?.1),
            None => None,
        };
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        self.call(Command::SetTransport {
            change: TransportChange {
                position: Some(a.start_beat.unwrap_or(0.0)),
                automation_write: Some(true),
                automation_mode: Some("write".into()),
                ..TransportChange::default()
            },
        })
        .await?;
        self.set_param(track, device, a.page.as_ref(), &a.param, a.from)
            .await?;
        if snap["transport"]["recording"] != true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        if let Some(scene) = scene {
            self.call(Command::SceneAction {
                scene,
                action: SceneAction::Launch,
            })
            .await?;
        } else {
            self.call(Command::Transport {
                action: TransportAction::Play,
            })
            .await?;
        }
        let begin = tokio::time::Instant::now();
        let mut last = Json::Null;
        for k in 1..=steps {
            tokio::time::sleep_until(
                begin + Duration::from_secs_f64(secs * f64::from(k) / f64::from(steps)),
            )
            .await;
            let t = shaped(f64::from(k) / f64::from(steps), curve);
            let v = a.from + (a.to - a.from) * t;
            last = self
                .set_param(track, device, a.page.as_ref(), &a.param, v)
                .await?;
        }
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        let after = self.call(Command::GetProject).await?;
        if after["transport"]["recording"] == true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        self.call(Command::SetTransport {
            change: TransportChange {
                automation_write: Some(false),
                ..TransportChange::default()
            },
        })
        .await?;
        out(&json!({"bars": a.bars, "steps": steps, "end": last, "curve": curve}))
    }

    #[tool(
        description = "Peak levels (0-1 on Bitwig's meter scale) of every track and the master. With window_ms the meters are cleared and read after that time: start playback, measure a bar or two, see which track is loud. For LUFS and true peak of the whole mix use capture_audio."
    )]
    async fn get_meters(&self, Parameters(a): Parameters<MetersArgs>) -> ToolResult {
        let ms = a.window_ms.unwrap_or(0);
        if ms > 60_000 {
            return Err(bad("window_ms must be 0-60000"));
        }
        let r = self
            .bridge
            .call_with(
                Command::GetMeters { window_ms: ms },
                Duration::from_millis(u64::from(ms) + 10_000),
            )
            .await
            .map_err(err)?;
        out(&r)
    }

    #[tool(
        description = "Put a mastering chain on the master track: EQ-5, Compressor and Peak Limiter by default (or your devices). Then measure with capture_audio (LUFS, true peak), adjust with set_device_params, and measure again."
    )]
    async fn master_chain(&self, Parameters(a): Parameters<MasterChainArgs>) -> ToolResult {
        let devices = a.devices.unwrap_or_else(|| {
            ["EQ-5", "Compressor", "Peak Limiter"]
                .into_iter()
                .map(|n| ChainDevice {
                    name: Some(n.into()),
                    preset: None,
                    params: Vec::new(),
                })
                .collect()
        });
        out(&self
            .load_chain(&Ref::Name("master".into()), &devices)
            .await?)
    }

    #[tool(
        description = "Device action: enable, disable, toggle, delete, duplicate, open_window, close_window, next_preset, previous_preset, select."
    )]
    async fn device_action(&self, Parameters(a): Parameters<DeviceActionArgs>) -> ToolResult {
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        out(&self
            .call(Command::DeviceAction {
                track,
                device,
                action: a.action,
            })
            .await?)
    }

    // ------------------------------------------------------------ MIDI and hardware

    #[tool(
        description = "Play notes on a track for a while (several notes = chord). On a HW Instrument track this plays the external synth."
    )]
    async fn play_notes(&self, Parameters(a): Parameters<PlayNotesArgs>) -> ToolResult {
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
    async fn send_midi(&self, Parameters(a): Parameters<SendMidiArgs>) -> ToolResult {
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
    async fn list_instruments(&self) -> ToolResult {
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
    async fn hardware_set(&self, Parameters(a): Parameters<HardwareSetArgs>) -> ToolResult {
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
    async fn hardware_program_change(
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
    async fn send_sysex(&self, Parameters(a): Parameters<SendSysexArgs>) -> ToolResult {
        let bytes = parse_sysex(&a.hex).map_err(bad)?;
        out(&self.call(Command::SendSysex { bytes }).await?)
    }

    // ------------------------------------------------------------ application

    #[tool(
        description = "Search Bitwig's application actions (every menu command: save, export, group tracks, consolidate, ...) by words in their id or name."
    )]
    async fn list_actions(&self, Parameters(a): Parameters<ListActionsArgs>) -> ToolResult {
        out(&self.call(Command::ListActions { query: a.query }).await?)
    }

    #[tool(
        description = "Invoke a Bitwig application action by id (from list_actions). Acts on the current selection, like the menu."
    )]
    async fn invoke_action(&self, Parameters(a): Parameters<InvokeActionArgs>) -> ToolResult {
        out(&self.call(Command::InvokeAction { id: a.id }).await?)
    }

    #[tool(description = "Undo or redo one or more steps.")]
    async fn history(&self, Parameters(a): Parameters<HistoryArgs>) -> ToolResult {
        let steps = a.steps.unwrap_or(1).clamp(1, 50);
        for _ in 0..steps {
            let c = match a.action {
                HistoryAction::Undo => Command::Undo,
                HistoryAction::Redo => Command::Redo,
            };
            self.call(c).await?;
        }
        out(&json!({"done": steps}))
    }

    #[tool(
        description = "Switch the window layout (arrange, mix, edit) and/or toggle a panel (inspector, devices, mixer, note_editor, automation_editor, browser, fullscreen)."
    )]
    async fn set_view(&self, Parameters(a): Parameters<SetViewArgs>) -> ToolResult {
        let layout = match a.layout.as_deref().map(str::to_lowercase).as_deref() {
            None => None,
            Some(l @ ("arrange" | "mix" | "edit")) => Some(l.to_uppercase()),
            Some(o) => {
                return Err(bad(format!(
                    "layout must be arrange, mix or edit, not {o:?}"
                )));
            }
        };
        out(&self
            .call(Command::SetView {
                layout,
                toggle: a.toggle,
            })
            .await?)
    }

    // ------------------------------------------------------------ composing

    #[tool(
        description = "Write a chord progression into a clip: chord symbols (\"Am7 Fmaj7 C G\") or Roman numerals in a key (key \"A minor\", \"i VI III VII\"), with voicing (close voice-leads smoothly), octave and an optional rhythm per chord (\"x..x..x.\"). An empty slot gets a new clip."
    )]
    async fn write_chords(&self, Parameters(a): Parameters<WriteChordsArgs>) -> ToolResult {
        let chords = chords_of(a.key.as_deref(), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rhythm = rhythm_of(a.rhythm.as_deref())?;
        let step = parse_step(a.step.as_deref(), 0.25)?;
        let notes = theory::chord_notes(
            &chords,
            per,
            a.octave.unwrap_or(3).clamp(-2, 8),
            a.voicing.unwrap_or_default(),
            rhythm.as_deref().map(|r| (r, step)),
            a.velocity.unwrap_or(90),
        );
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Write a bassline that follows a chord progression (symbols or Roman numerals in a key): style root, octave, fifth, arpeggio or walking; rhythm as steps (default \"x.xx\" in eighths); octave 1 by default."
    )]
    async fn write_bassline(&self, Parameters(a): Parameters<WriteBasslineArgs>) -> ToolResult {
        let chords = chords_of(a.key.as_deref(), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rhythm = rhythm_of(a.rhythm.as_deref())?;
        let step = parse_step(a.step.as_deref(), 0.5)?;
        let notes = theory::bassline(
            &chords,
            per,
            a.octave.unwrap_or(1).clamp(-2, 8),
            a.style.unwrap_or_default(),
            rhythm.as_deref().map(|r| (r, step)),
            a.velocity.unwrap_or(100),
        );
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Write an arpeggio over a chord progression: direction up, down, up_down or random (seeded), rate (\"1/16\", \"1/8T\"), octaves and gate."
    )]
    async fn write_arp(&self, Parameters(a): Parameters<WriteArpArgs>) -> ToolResult {
        let chords = chords_of(a.key.as_deref(), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rate = parse_step(a.rate.as_deref(), 0.25)?;
        let notes = theory::arpeggio(
            &chords,
            per,
            a.octave.unwrap_or(3).clamp(-2, 8),
            a.direction.unwrap_or_default(),
            rate,
            a.octaves.unwrap_or(1).clamp(1, 4),
            a.gate.unwrap_or(0.8),
            a.velocity.unwrap_or(90),
            a.seed.unwrap_or(1),
        );
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Write a melodic hook in a key over a chord progression: a short motif (rhythm + contour: arch, rise, fall, wave, flat) repeated through the clip with small variations, mostly stepwise, landing on chord tones at chord changes and at the end. Tune density, rate, span, variation; the seed gives other hooks. Run it again with another seed or transform_clip for alternatives."
    )]
    async fn write_melody(&self, Parameters(a): Parameters<WriteMelodyArgs>) -> ToolResult {
        let scale = theory::Scale::parse(&a.key).map_err(bad)?;
        let chords = chords_of(Some(&a.key), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rate = parse_step(a.rate.as_deref(), 0.5)?;
        let motif = a.motif_beats.unwrap_or(4.0);
        if !(motif.is_finite() && (0.5..=32.0).contains(&motif)) {
            return Err(bad("motif_beats must be 0.5-32"));
        }
        let rhythm = a
            .rhythm
            .as_deref()
            .map(theory::steps)
            .transpose()
            .map_err(bad)?;
        let spec = theory::MelodySpec {
            beats_per_chord: per,
            octave: a.octave.unwrap_or(4).clamp(-2, 8),
            span: a.span.unwrap_or(1).clamp(1, 3),
            contour: a.contour.unwrap_or_default(),
            density: a.density.unwrap_or(0.6).clamp(0.05, 1.0),
            rate,
            motif_beats: motif,
            rhythm,
            variation: a.variation.unwrap_or(0.3).clamp(0.0, 1.0),
            velocity: a.velocity.unwrap_or(95),
            seed: a.seed.unwrap_or(1),
        };
        let notes = theory::melody(&scale, &chords, &spec);
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Build a song from its plan in one call: for every section, run its parts (drums, bass, chords, arp, melody, clip, copy) into the section's scene. Uses the sections of the stored song plan unless given. Each part is a write_* / create_clip / copy_clip call; a failing part is reported and the rest still run. Create the tracks and scenes first."
    )]
    async fn build_song(&self, Parameters(a): Parameters<BuildSongArgs>) -> ToolResult {
        let plan: Json = match &a.sections {
            Some(_) => Json::Null,
            None => {
                let r = self.call(Command::GetSongPlan).await?;
                serde_json::from_str(r["text"].as_str().unwrap_or_default()).unwrap_or(Json::Null)
            }
        };
        let sections = a
            .sections
            .clone()
            .or_else(|| plan["sections"].as_array().cloned())
            .ok_or_else(|| bad("no sections: pass them, or store a plan with sections and parts (set_song_plan)"))?;
        let key = plan["key"].as_str().map(str::to_owned);
        let mut results = Vec::new();
        for sec in &sections {
            let name = sec["name"].as_str().unwrap_or_default();
            if let Some(only) = &a.only
                && !only.iter().any(|o| o.eq_ignore_ascii_case(name))
            {
                continue;
            }
            let scene = sec["scene"].as_str().unwrap_or(name);
            for part in sec["parts"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let mut part = part.clone();
                let kind = part["kind"].as_str().unwrap_or_default().to_owned();
                if let Some(o) = part.as_object_mut() {
                    o.remove("kind");
                    if kind != "copy" && !scene.is_empty() {
                        o.entry("slot").or_insert_with(|| json!(scene));
                    }
                    if let (Some(k), true) = (
                        &key,
                        matches!(kind.as_str(), "bass" | "chords" | "arp" | "melody"),
                    ) {
                        o.entry("key").or_insert_with(|| json!(k));
                    }
                }
                let track = part["track"].clone();
                let r = self.build_part(&kind, part).await;
                results.push(match r {
                    Ok(_) => json!({"section": name, "kind": kind, "track": track, "ok": true}),
                    Err(e) => {
                        json!({"section": name, "kind": kind, "track": track, "error": e.message})
                    }
                });
            }
        }
        if results.is_empty() {
            return Err(bad(
                "no parts to build (check `only` and the sections' parts)",
            ));
        }
        let failed = results.iter().filter(|r| r.get("error").is_some()).count();
        out(&json!({"built": results.len() - failed, "failed": failed, "parts": results}))
    }

    #[tool(
        description = "Write a drum pattern: per voice a step string (\"x...x...x...x...\", X = accent, _ = tie) and/or a Euclidean rhythm. Voices are named keys of an instrument profile (default General MIDI, which Bitwig's Drum Machine uses: kick, snare, clap, closed_hat, open_hat, low_tom, mid_tom, high_tom, crash, ride), note names or numbers."
    )]
    async fn write_drums(&self, Parameters(a): Parameters<WriteDrumsArgs>) -> ToolResult {
        if a.patterns.is_empty() && a.euclid.is_empty() {
            return Err(bad("give patterns and/or euclid"));
        }
        let registry = self.profiles();
        let pname = a.instrument.as_deref().unwrap_or("generic");
        let profile = registry
            .get(pname)
            .ok_or_else(|| bad(format!("no instrument profile {pname:?}")))?;
        let step = parse_step(a.step.as_deref(), 0.25)?;
        let length = a.length_beats.unwrap_or(4.0);
        if !(length.is_finite() && (0.25..=256.0).contains(&length)) {
            return Err(bad("length_beats must be 0.25-256"));
        }
        let mut voices = Vec::new();
        for (voice, pattern) in &a.patterns {
            voices.push((
                voice_key(Some(profile), voice)?,
                theory::steps(pattern).map_err(bad)?,
            ));
        }
        for (voice, e) in &a.euclid {
            if e.steps == 0 || e.steps > 64 {
                return Err(bad("euclid steps must be 1-64"));
            }
            voices.push((
                voice_key(Some(profile), voice)?,
                theory::euclid(e.hits, e.steps, e.rotate.unwrap_or(0)),
            ));
        }
        let notes = theory::drums(&voices, step, length, a.velocity.unwrap_or(100));
        self.put_notes(&a.target, notes, length).await
    }

    #[tool(
        description = "Transform the notes of a clip in place: transpose, to_scale (snap to a key), humanize (seeded timing/velocity), quantize, reverse, stretch (0.5 = double time, 2 = half time), velocity (scale/offset), thin. Ops run in order."
    )]
    async fn transform_clip(&self, Parameters(a): Parameters<TransformClipArgs>) -> ToolResult {
        if a.ops.is_empty() {
            return Err(bad("ops is empty"));
        }
        let (_, (track, slot)) = self
            .resolve(|v| {
                let track = v.track(&a.track)?;
                let slot = v.slot(&a.slot)?;
                if v.has_clip(track, slot) {
                    Ok((track, slot))
                } else {
                    Err(format!("slot {} has no clip", slot + 1))
                }
            })
            .await?;
        let (notes, length) = self.read_clip_notes(track, slot).await?;
        if notes.is_empty() {
            return Err(bad("the clip has no notes"));
        }
        let changed = theory::transform(&notes, &a.ops, length).map_err(bad)?;
        let target = Target {
            track: a.track,
            slot: Ref::Number(slot + 1),
            name: None,
            mode: Some(WriteMode::Replace),
        };
        let r = self.put_notes(&target, changed, length).await?;
        out(
            &json!({"before": notes.len(), "result": serde_json::from_str::<Json>(&r).unwrap_or(Json::Null)}),
        )
    }

    #[tool(
        description = "Copy a clip's notes into other slots (scenes), on the same track or another, each copy optionally varied (transpose, thin, humanize, ...). Replaces what is in the target slots. Saves rewriting the same pad for every section."
    )]
    async fn copy_clip(&self, Parameters(a): Parameters<CopyClipArgs>) -> ToolResult {
        if a.to.is_empty() {
            return Err(bad("to is empty"));
        }
        let (_, (track, slot)) = self
            .resolve(|v| {
                let track = v.track(&a.track)?;
                let slot = v.slot(&a.slot)?;
                if v.has_clip(track, slot) {
                    Ok((track, slot))
                } else {
                    Err(format!("slot {} has no clip", slot + 1))
                }
            })
            .await?;
        let (notes, length) = self.read_clip_notes(track, slot).await?;
        if notes.is_empty() {
            return Err(bad("the source clip has no notes"));
        }
        let dest_track = a.to_track.clone().unwrap_or_else(|| a.track.clone());
        let mut done = Vec::new();
        for d in &a.to {
            let copy = match d.ops.as_deref() {
                Some(ops) if !ops.is_empty() => {
                    theory::transform(&notes, ops, length).map_err(bad)?
                }
                _ => notes.clone(),
            };
            if copy.is_empty() {
                return Err(bad(format!("the variation for {} left no notes", d.slot)));
            }
            let target = Target {
                track: dest_track.clone(),
                slot: d.slot.clone(),
                name: d.name.clone(),
                mode: Some(WriteMode::Replace),
            };
            self.put_notes(&target, copy.clone(), length).await?;
            done.push(json!({"slot": d.slot, "notes": copy.len()}));
        }
        out(&json!({"source_notes": notes.len(), "length_beats": length, "copied": done}))
    }

    #[tool(
        description = "Record a launcher clip for a number of bars at the project tempo, then play or stop it. For a hardware synth: record on an audio track whose input is the synth, and pass notes + play_on (its HW Instrument track) to play a part into the recording. The call returns when recording is done."
    )]
    async fn record_clip(&self, Parameters(a): Parameters<RecordClipArgs>) -> ToolResult {
        if !(1..=64).contains(&a.bars) {
            return Err(bad("bars must be 1-64"));
        }
        let then_play = match a.then.as_deref() {
            None | Some("play") => true,
            Some("stop") => false,
            Some(o) => return Err(bad(format!("then must be play or stop, not {o:?}"))),
        };
        let (snap, view) = self.project().await?;
        let track = view.track(&a.track).map_err(bad)?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        if view.has_clip(track, slot) {
            return Err(bad(format!("slot {} already holds a clip", slot + 1)));
        }
        let player = a
            .play_on
            .as_ref()
            .map(|r| view.track(r))
            .transpose()
            .map_err(bad)?;
        if !a.notes.is_empty() && player.is_none() {
            return Err(bad("notes need play_on: the track that plays them"));
        }
        let tempo = snap["transport"]["tempo"]
            .as_f64()
            .unwrap_or(120.0)
            .max(20.0);
        let beats_per_bar = snap["transport"]["time_signature"]
            .as_str()
            .and_then(|s| s.split_once('/'))
            .and_then(|(n, d)| {
                Some(n.trim().parse::<f64>().ok()? * 4.0 / d.trim().parse::<f64>().ok()?)
            })
            .unwrap_or(4.0);
        let beats = f64::from(a.bars) * beats_per_bar;
        let seconds = beats * 60.0 / tempo;
        let keys: Vec<(f64, f64, u8, u8)> = a
            .notes
            .iter()
            .map(|n| Ok((n.start, n.length, n.note.key()?, n.velocity.unwrap_or(100))))
            .collect::<Result<_, McpError>>()?;
        self.call(Command::SetTrack {
            track,
            change: TrackChange {
                arm: Some(true),
                ..TrackChange::default()
            },
        })
        .await?;
        self.call(Command::ClipAction {
            track,
            slot,
            action: ClipAction::Record,
        })
        .await?;
        // wait for the launch quantization to start the recording
        let started = std::time::Instant::now();
        loop {
            let p = self.call(Command::GetProject).await?;
            if track_clip(&p, track, slot)["recording"] == true {
                break;
            }
            if started.elapsed() > Duration::from_secs(20) {
                return Err(err(
                    "recording did not start within 20 s (is the transport playing or the launch quantization long?)",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let t0 = tokio::time::Instant::now();
        if let Some(p) = player {
            for (start, len, key, vel) in &keys {
                let at = t0 + Duration::from_secs_f64((start * 60.0 / tempo).max(0.0));
                tokio::time::sleep_until(at).await;
                let ms = (len * 60_000.0 / tempo).clamp(10.0, 10_000.0) as u32;
                let call = Command::PlayNotes {
                    track: p,
                    keys: vec![*key],
                    velocity: *vel,
                    duration_ms: ms,
                };
                let bridge = Arc::clone(&self.bridge);
                tokio::spawn(async move { bridge.call(call).await });
            }
        }
        tokio::time::sleep_until(t0 + Duration::from_secs_f64(seconds)).await;
        let finish = if then_play {
            ClipAction::Launch
        } else {
            ClipAction::Stop
        };
        self.call(Command::ClipAction {
            track,
            slot,
            action: finish,
        })
        .await?;
        self.call(Command::SetTrack {
            track,
            change: TrackChange {
                arm: Some(false),
                ..TrackChange::default()
            },
        })
        .await?;
        out(
            &json!({"recorded_bars": a.bars, "seconds": seconds, "slot": slot + 1, "then": if then_play { "play" } else { "stop" }}),
        )
    }

    // ------------------------------------------------------------ sound

    #[tool(
        description = "List what Bitwig has installed: every device (or preset) in the browser, each Bitwig device annotated with its role and sound tips from the built-in guide. Use it before choosing instruments and effects."
    )]
    async fn list_devices(&self, Parameters(a): Parameters<ListDevicesArgs>) -> ToolResult {
        // the browser offers what fits the track: instruments only on tracks that can hold them
        let view = self.view().await?;
        let track = view.tracks.first().map(TrackView::addr).ok_or_else(|| {
            bad("create a track first: Bitwig lists instruments only for a track that can hold them")
        })?;
        let r = self
            .call_long(Command::ListBrowser {
                track,
                content_type: a.content_type.clone().unwrap_or_else(|| "Devices".into()),
                max: a.max.unwrap_or(2000).clamp(1, 2000),
            })
            .await?;
        let needle = a.query.as_deref().map(str::to_lowercase);
        let items: Vec<Json> = r["names"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(Json::as_str)
                    .filter(|n| needle.as_ref().is_none_or(|q| n.to_lowercase().contains(q)))
                    .map(|n| match crate::devices::find(n) {
                        Some(g) => {
                            json!({"name": n, "kind": g.kind, "role": g.role, "tips": g.tips})
                        }
                        None => json!({"name": n}),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Bitwig's own devices first, then everything else (presets, plugins)
        let (mut known, other): (Vec<Json>, Vec<Json>) =
            items.into_iter().partition(|i| i.get("kind").is_some());
        let other_names: Vec<Json> = other.into_iter().map(|i| i["name"].clone()).collect();
        // presets run into the thousands: names only when searched for
        let presets = if needle.is_some() {
            json!(other_names)
        } else {
            json!(format!(
                "{} presets and plugins; pass query to search them",
                other_names.len()
            ))
        };
        known.sort_by_key(|i| i["kind"].as_str().unwrap_or_default().to_owned());
        out(&json!({
            "total_in_browser": r["total"],
            "bitwig_devices": known,
            "presets_and_plugins": presets,
        }))
    }

    #[tool(
        description = "Fill a Drum Machine's pads with instruments and add effects on single pads (effects {\"snare\": [\"Gate\", \"Reverb\"]}: the gated 80s snare), e.g. pads {\"kick\": \"v8 Kick\", \"snare\": \"v8 Snare\", \"closed_hat\": \"v8 Hat\"}. Pad names are the General MIDI keys write_drums uses, so a kit built here plays the drum patterns written there."
    )]
    async fn build_drum_kit(&self, Parameters(a): Parameters<DrumKitArgs>) -> ToolResult {
        if (a.pads.is_empty() && a.effects.is_empty()) || a.pads.len() > 32 {
            return Err(bad("give 1-32 pads and/or effects"));
        }
        let dref = a.device.unwrap_or_else(|| Ref::Name("Drum Machine".into()));
        let (track, device) = self.track_device(&a.track, &dref).await?;
        let registry = self.profiles();
        let gm = registry.get("generic");
        let mut plan = Vec::new();
        for (pad, name) in &a.pads {
            plan.push((voice_key(gm, pad)?, pad.clone(), name.clone()));
        }
        let mut done = Vec::new();
        for (key, pad, name) in plan {
            match self
                .call_long(Command::DrumPadInsert {
                    track,
                    device,
                    key,
                    source: DeviceSource::Browser {
                        name: name.clone(),
                        content_type: None,
                    },
                    append: false,
                })
                .await
            {
                Ok(_) => done.push(json!({"pad": pad, "key": key, "device": name})),
                Err(e) => done.push(json!({"pad": pad, "key": key, "error": e.message})),
            }
        }
        let mut fx_done = Vec::new();
        for (pad, names) in &a.effects {
            let key = voice_key(gm, pad)?;
            for name in names {
                match self
                    .call_long(Command::DrumPadInsert {
                        track,
                        device,
                        key,
                        source: DeviceSource::Browser {
                            name: name.clone(),
                            content_type: None,
                        },
                        append: true,
                    })
                    .await
                {
                    Ok(_) => fx_done.push(json!({"pad": pad, "effect": name})),
                    Err(e) => fx_done.push(json!({"pad": pad, "effect": name, "error": e.message})),
                }
            }
        }
        // the cursor device moves into the pad it filled: ask for the drum machine itself
        let machine = self
            .call(Command::GetDevice {
                track,
                device,
                page: None,
            })
            .await
            .map(|d| d["drum_pads"].clone())
            .unwrap_or(Json::Null);
        out(&json!({"pads": done, "effects": fx_done, "drum_pads_now": machine}))
    }

    // ------------------------------------------------------------ export

    #[tool(
        description = "Export clips as a standard MIDI file (.mid): every clip of a scene (one MIDI track per Bitwig track) or a list of clips. Writes into the music folder's bitwig-mcp-rs/ unless a path is given."
    )]
    async fn export_midi(&self, Parameters(a): Parameters<ExportMidiArgs>) -> ToolResult {
        let (snap, view) = self.project().await?;
        let mut picks: Vec<(TrackAddr, u32)> = Vec::new();
        if let Some(scene) = &a.scene {
            let slot = view.slot(scene).map_err(bad)?;
            for t in view.tracks.iter().chain(&view.effect_tracks) {
                if t.clips.iter().any(|c| c.slot == slot) {
                    picks.push((t.addr(), slot));
                }
            }
        }
        for c in &a.clips {
            let track = view.track(&c.track).map_err(bad)?;
            let slot = view.slot(&c.slot).map_err(bad)?;
            if !view.has_clip(track, slot) {
                return Err(bad(format!("slot {} of {} has no clip", slot + 1, c.track)));
            }
            picks.push((track, slot));
        }
        if picks.is_empty() {
            return Err(bad("nothing to export: give a scene with clips or clips"));
        }
        let mut tracks = Vec::new();
        for (track, slot) in &picks {
            let (notes, _) = self.read_clip_notes(*track, *slot).await?;
            let name = view
                .track_view(*track)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            tracks.push(crate::smf::SmfTrack {
                name,
                channel: 1,
                notes,
            });
        }
        let (tempo, _, num, den) = tempo_of(&snap);
        let project = snap["application"]["project_name"]
            .as_str()
            .unwrap_or("bitwig");
        let stem = format!("{project}-{}", capture::stamp());
        let path = export_path(a.path.as_deref(), &stem, "mid");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let bytes = crate::smf::write(&tracks, tempo, num, den);
        std::fs::write(&path, &bytes).map_err(|e| err(format!("{}: {e}", path.display())))?;
        out(&json!({
            "path": path.display().to_string(),
            "tracks": tracks.iter().map(|t| json!({"name": t.name, "notes": t.notes.len()})).collect::<Vec<_>>(),
            "tempo": tempo,
        }))
    }

    #[tool(
        description = "Export audio by recording Bitwig's output in real time (Linux, PipeWire): plays the arranger from the start, or a scene from bar 1, for N bars and writes WAV, FLAC or MP3 into the music folder's bitwig-mcp-rs/. Optional stems (each track alone, one pass each). Reports loudness (LUFS, true peak) with a short verdict, so you can check the mix."
    )]
    async fn capture_audio(&self, Parameters(a): Parameters<CaptureArgs>) -> ToolResult {
        let bars = a.bars.unwrap_or(8);
        if !(1..=512).contains(&bars) {
            return Err(bad("bars must be 1-512"));
        }
        let tail = a.tail_seconds.unwrap_or(2.0).clamp(0.0, 30.0);
        let format = a.format.unwrap_or_default();
        let (snap, view) = self.project().await?;
        let scene = a
            .scene
            .as_ref()
            .map(|s| view.scene(s))
            .transpose()
            .map_err(bad)?;
        let (tempo, beats_per_bar, _, _) = tempo_of(&snap);
        let seconds = f64::from(bars) * beats_per_bar * 60.0 / tempo;
        let project = snap["application"]["project_name"]
            .as_str()
            .unwrap_or("bitwig")
            .to_owned();
        let base = export_path(
            a.path.as_deref(),
            &format!("{project}-{}", capture::stamp()),
            format.ext(),
        );

        // passes: the mix, then one per track with clips when stems are asked for
        let mut passes: Vec<(String, Option<TrackAddr>)> = vec![("mix".into(), None)];
        if a.stems.unwrap_or(false) {
            for t in &view.tracks {
                if !t.clips.is_empty() || !t.devices.is_empty() {
                    passes.push((t.name.clone(), Some(t.addr())));
                }
            }
        }
        let mut files = Vec::new();
        for (label, solo) in passes {
            let file = if solo.is_some() {
                let stem = base
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                base.with_file_name(format!("{stem}-{}", capture::safe_name(&label)))
                    .with_extension(format.ext())
            } else {
                base.clone()
            };
            if let Some(t) = solo {
                self.call(Command::SetTrack {
                    track: t,
                    change: TrackChange {
                        solo: Some(true),
                        ..TrackChange::default()
                    },
                })
                .await?;
            }
            let wav = file.with_extension("capture.wav");
            let bridge = Arc::clone(&self.bridge);
            let result = capture::record(
                capture::BITWIG_NODE,
                &wav,
                Duration::from_secs_f64(tail),
                || async move {
                    bridge
                        .call(Command::Transport {
                            action: TransportAction::Stop,
                        })
                        .await?;
                    bridge
                        .call(Command::SetTransport {
                            change: TransportChange {
                                position: Some(0.0),
                                ..TransportChange::default()
                            },
                        })
                        .await?;
                    if let Some(sc) = scene {
                        bridge
                            .call(Command::SceneAction {
                                scene: sc,
                                action: SceneAction::Launch,
                            })
                            .await?;
                    }
                    bridge
                        .call(Command::Transport {
                            action: TransportAction::Play,
                        })
                        .await?;
                    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                    bridge
                        .call(Command::Transport {
                            action: TransportAction::Stop,
                        })
                        .await?;
                    Ok(())
                },
            )
            .await;
            if let Some(t) = solo {
                self.call(Command::SetTrack {
                    track: t,
                    change: TrackChange {
                        solo: Some(false),
                        ..TrackChange::default()
                    },
                })
                .await?;
            }
            result.map_err(err)?;
            // the stream starts 0.4 s before the music: trim it
            capture::encode(&wav, &file, format, 0.4)
                .await
                .map_err(err)?;
            let loudness = capture::measure(&file).await.unwrap_or_default();
            files.push(json!({
                "part": label,
                "path": file.display().to_string(),
                "loudness": loudness,
                "verdict": loudness.verdict(),
            }));
        }
        out(&json!({"bars": bars, "seconds": seconds + tail, "files": files}))
    }

    #[tool(
        description = "Save the project (Bitwig's Save; an unsaved project asks for a name in Bitwig)."
    )]
    async fn save_project(&self) -> ToolResult {
        out(&self
            .call(Command::InvokeAction { id: "Save".into() })
            .await?)
    }

    #[tool(
        description = "Open one of Bitwig's export dialogs for the user to finish: audio (offline render with all options), midi or dawproject (for other DAWs)."
    )]
    async fn open_export_dialog(&self, Parameters(a): Parameters<ExportDialogArgs>) -> ToolResult {
        let id = match a.kind {
            ExportKind::Audio => "Export Audio",
            ExportKind::Midi => "export_midi",
            ExportKind::Dawproject => "export_project",
        };
        out(&self.call(Command::InvokeAction { id: id.into() }).await?)
    }

    // ------------------------------------------------------------ song plan

    #[tool(
        description = "The song plan stored in this Bitwig project: key, tempo, sections, track roles and ideas from earlier sessions. Read it before continuing a song."
    )]
    async fn get_song_plan(&self) -> ToolResult {
        let r = self.call(Command::GetSongPlan).await?;
        let text = r["text"].as_str().unwrap_or_default();
        if text.trim().is_empty() {
            return out(
                &json!({"plan": null, "hint": "no plan yet: set_song_plan after deciding key, tempo and sections"}),
            );
        }
        let plan: Json = serde_json::from_str(text).unwrap_or(Json::String(text.to_owned()));
        out(&json!({"plan": plan}))
    }

    #[tool(
        description = "Store the song plan (JSON) in the Bitwig project, so later sessions can continue the song: title, genre, key, tempo, sections with bars and scenes, track roles, decisions. Replaces the stored plan; saved with the project."
    )]
    async fn set_song_plan(&self, Parameters(a): Parameters<SongPlanArgs>) -> ToolResult {
        let text = serde_json::to_string(&a.plan).map_err(err)?;
        if text.chars().count() > 60_000 {
            return Err(bad("the plan is limited to 60000 characters"));
        }
        out(&self.call(Command::SetSongPlan { text }).await?)
    }
}

#[prompt_router]
impl BridgeServer {
    #[prompt(
        description = "Compose a track step by step with the composing tools, asking the user at each stage. Keeps the song plan in the project."
    )]
    async fn compose_track(&self, Parameters(a): Parameters<ComposeArgs>) -> Vec<PromptMessage> {
        let hardware = a
            .hardware
            .map(|h| format!("Hardware to use: {h}. Check list_instruments for their channels and what they can do; play them through HW Instrument tracks.\n"))
            .unwrap_or_default();
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Compose with me in Bitwig: {idea}.\n{hardware}\
                 Work in stages and after each one tell me what you did and ask: keep, vary or redo?\n\
                 0. get_song_plan: if a plan exists, continue it instead of starting over. get_project to see what exists; never delete my material.\n\
                 1. Concept: pick key, tempo, time signature and 3-5 sections (e.g. intro 8 bars, build 8, drop 16, break 8, outro 8). set_transport tempo, then set_song_plan with all of it.\n\
                 2. Drums: create_track instrument \"Drums\" with device \"Drum Machine\"; write_drums per section (vary hats and fills; keep the kick of the genre).\n\
                 3. Bass: write_bassline following the chord progression of each section.\n\
                 4. Harmony: write_chords (voicing close for pads, rhythm for stabs) on a pad or keys track.\n\
                 5. Lead / arp: write_arp or a melody with create_clip notes in the key; leave space for the vocal or hook.\n\
                 6. Sections: create_scene per section and put each part's clip in the section's slot (slot = scene name). Use transform_clip for variations (transpose, thin, humanize) instead of rewriting.\n\
                 7. Mix: set_track volumes in dB (drums and bass loudest, pads -10 dB or lower), pan supporting parts, sends to an effect track with reverb.\n\
                 8. Update set_song_plan with what was built, then launch the first scene.\n\
                 Keep every part in the key ({{key}} from the plan); use Roman numerals so changes of key stay easy.",
                idea = a.idea
            ),
        )]
    }

    #[prompt(
        description = "Sketch a song: tracks with instruments, scenes for the sections, and clips with notes."
    )]
    async fn sketch_song(&self, Parameters(a): Parameters<SketchArgs>) -> Vec<PromptMessage> {
        let tempo = a.tempo.unwrap_or_else(|| "a fitting tempo".into());
        let key = a.key.unwrap_or_else(|| "a fitting key".into());
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Sketch a {genre} track in Bitwig at {tempo} in {key}.\n\
                 1. get_project to see what exists; don't delete anything.\n\
                 2. set_transport tempo.\n\
                 3. create_track for drums, bass, chords and a lead, each with a fitting Bitwig \
                 device (Drum Machine, Polymer, Phase-4, ...), a name and a color.\n\
                 4. create_scene Intro, Verse, Chorus, Outro.\n\
                 5. create_clip per track and scene with idiomatic {genre} notes (4 or 8 bars).\n\
                 6. Launch the Verse scene, then summarize what you built.",
                genre = a.genre
            ),
        )]
    }

    #[prompt(description = "Check the mix: levels, panning, mutes and solos, with concrete fixes.")]
    async fn mix_check(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            "Read get_project and review the mix: tracks far above or below the others, \
             everything panned center, forgotten solos/mutes, sends without effect tracks, a \
             master above 0 dB. Propose concrete changes (in dB) and apply them with set_track \
             only after I agree.",
        )]
    }

    #[prompt(
        description = "Set up an external hardware synth: HW Instrument track, test notes, and an instrument profile."
    )]
    async fn setup_hardware_synth(
        &self,
        Parameters(a): Parameters<HardwarePromptArgs>,
    ) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Help me use my {synth} from Bitwig.\n\
                 1. list_instruments: if a profile for the {synth} exists, use its channel, \
                 notes and parameters; otherwise write one (TOML, from its MIDI implementation \
                 chart) for the instruments folder that list_instruments reports, and check it loads.\n\
                 2. create_track kind instrument named \"{synth}\" with device \"HW Instrument\".\n\
                 3. Tell me to pick the MIDI output port, the MIDI channel from the profile and \
                 the audio input in that device, and to set the synth to the same channel.\n\
                 4. play_notes a chord (a single note for mono synths) to test.\n\
                 5. Try a hardware_set parameter from the profile, if it has any, and explain \
                 which knobs stay manual.",
                synth = a.synth
            ),
        )]
    }
}

const RESOURCES: [(&str, &str, &str); 4] = [
    (
        "bitwig://devices",
        "devices",
        "Guide to Bitwig's own devices: role and sound recipes (list_devices shows what is installed).",
    ),
    (
        "bitwig://project",
        "project",
        "The whole Bitwig project as JSON (same as get_project).",
    ),
    (
        "bitwig://instruments",
        "instruments",
        "Hardware instrument profiles.",
    ),
    (
        "bitwig://protocol",
        "protocol",
        "JSON Schema of the bridge protocol.",
    ),
];

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for BridgeServer {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        let _turn = self.turn.lock().await;
        let call = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(call).await
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            "bitwig-mcp-rs",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources: Vec<Resource> = RESOURCES
            .iter()
            .map(|(uri, name, desc)| {
                Resource::new(*uri, *name)
                    .with_description(*desc)
                    .with_mime_type("application/json")
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let text = match request.uri.as_str() {
            "bitwig://project" => {
                self.get_project(Parameters(ProjectArgs { detail: Some(true) }))
                    .await?
            }
            "bitwig://instruments" => self.list_instruments().await?,
            "bitwig://devices" => {
                serde_json::to_string_pretty(crate::devices::GUIDE).map_err(err)?
            }
            "bitwig://protocol" => {
                serde_json::to_string_pretty(&super::protocol::schema()).map_err(err)?
            }
            _ => return Err(McpError::resource_not_found(request.uri, None)),
        };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::fake;
    use super::*;

    fn snapshot() -> Json {
        json!({
            "application": {"project_name": "Demo"},
            "transport": {"playing": false, "tempo": 666.0},
            "tracks": [
                {"bank": "main", "index": 0, "name": "Drums",
                 "devices": [{"index": 0, "name": "Drum Machine"}],
                 "sends": [{"index": 0, "name": "Reverb"}]},
                {"bank": "main", "index": 1, "name": "Synth",
                 "devices": [{"index": 0, "name": "HW Instrument"}],
                 "clips": [{"slot": 0, "name": "pad"}]}
            ],
            "effect_tracks": [{"bank": "effect", "index": 0, "name": "Reverb"}],
            "master": {"bank": "master", "index": 0, "name": "Master"},
            "scenes": [{"index": 0, "name": "Intro"}, {"index": 1, "name": "Verse"}]
        })
    }

    /// A server on a fake bridge that answers get_project with [`snapshot`] and echoes the rest.
    async fn rig() -> (BridgeServer, Arc<std::sync::Mutex<Vec<Json>>>) {
        let (port, log) = fake::spawn(|req| match req["cmd"].as_str() {
            Some("get_project") => Ok(snapshot()),
            Some("create_track") => Ok(json!({"bank": "main", "index": 2})),
            Some("create_scene") => Ok(json!({"index": 2})),
            Some("read_notes") => Ok(json!({
                "clip": {"loop_length": 4.0},
                "notes": [
                    {"x": 0, "key": 60, "velocity": 100, "duration": 0.5},
                    {"x": 32, "key": 64, "velocity": 80, "duration": 0.5}
                ]
            })),
            Some("get_song_plan") => Ok(json!({"text": "{\"key\":\"A minor\"}"})),
            Some("get_device") => Ok(json!({
                "pages": ["Main", "Filter"],
                // a page with a gap, like Bitwig's Test Tone (parameters 1, 2 and 8)
                "params": [{"name": "Cutoff", "index": 0}, {"name": "Resonance", "index": 7}]
            })),
            Some("insert_device") if req["source"]["name"] == "Nope" => Err("not found".into()),
            _ => Ok(req.clone()),
        })
        .await;
        let dir = std::env::temp_dir().join("bwmcp-no-profiles");
        (
            BridgeServer::new(Arc::new(Bridge::new("127.0.0.1", port)), dir),
            log,
        )
    }

    fn sent(log: &std::sync::Mutex<Vec<Json>>, cmd: &str) -> Vec<Json> {
        log.lock()
            .expect("log")
            .iter()
            .filter(|r| r["cmd"] == cmd)
            .cloned()
            .collect()
    }

    #[test]
    fn steps_and_rates() {
        assert_eq!(parse_step(None, 0.25).ok(), Some(0.25));
        assert_eq!(parse_step(Some("1/16"), 1.0).ok(), Some(0.25));
        assert_eq!(parse_step(Some("1/8"), 1.0).ok(), Some(0.5));
        assert!((parse_step(Some("1/8T"), 1.0).expect("triplet") - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(parse_step(Some("2"), 1.0).ok(), Some(2.0));
        assert!(parse_step(Some("1/1024"), 1.0).is_err());
        assert!(parse_step(Some("fast"), 1.0).is_err());
    }

    #[test]
    fn drum_voices_resolve_names_notes_and_numbers() {
        let reg = Registry::with_builtin();
        let gm = reg.get("generic");
        assert_eq!(voice_key(gm, "kick").ok(), Some(36));
        assert_eq!(voice_key(gm, "Closed Hat").ok(), Some(42));
        assert_eq!(voice_key(gm, "C1").ok(), Some(36));
        assert_eq!(voice_key(None, "38").ok(), Some(38));
        assert!(
            voice_key(gm, "cowbell-ish")
                .unwrap_err()
                .message
                .contains("kick")
        );
    }

    #[tokio::test]
    async fn write_chords_creates_a_clip_and_writes_voiced_chords() {
        let (s, log) = rig().await;
        let r = s
            .write_chords(Parameters(WriteChordsArgs {
                target: Target {
                    track: Ref::Name("synth".into()),
                    slot: Ref::Name("verse".into()),
                    name: Some("chords".into()),
                    mode: None,
                },
                key: Some("A minor".into()),
                progression: "i VI III VII".into(),
                beats_per_chord: None,
                octave: None,
                voicing: None,
                rhythm: None,
                step: None,
                velocity: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"new_clip\":true"), "{r}");
        assert_eq!(sent(&log, "create_clip")[0]["length"], 16);
        let w = &sent(&log, "write_notes")[0];
        assert_eq!(w["notes"].as_array().map(Vec::len), Some(12), "4 triads");
        assert_eq!(w["step"], 1.0, "chords on the coarsest (beat) grid");
        assert_eq!(sent(&log, "set_clip")[0]["name"], "chords");
    }

    #[tokio::test]
    async fn write_drums_maps_voices_to_general_midi_keys() {
        let (s, log) = rig().await;
        let mut patterns = std::collections::BTreeMap::new();
        patterns.insert("kick".to_owned(), "x...x...x...x...".to_owned());
        let mut euclid = std::collections::BTreeMap::new();
        euclid.insert(
            "closed_hat".to_owned(),
            EuclidArg {
                hits: 5,
                steps: 8,
                rotate: None,
            },
        );
        s.write_drums(Parameters(WriteDrumsArgs {
            target: Target {
                track: Ref::Number(1),
                slot: Ref::Number(1),
                name: None,
                mode: None,
            },
            instrument: None,
            patterns,
            euclid,
            step: None,
            length_beats: None,
            velocity: None,
        }))
        .await
        .expect("tool");
        let notes = sent(&log, "write_notes")[0]["notes"].clone();
        let keys: Vec<u64> = notes
            .as_array()
            .expect("notes")
            .iter()
            .filter_map(|n| n["key"].as_u64())
            .collect();
        assert_eq!(keys.iter().filter(|k| **k == 36).count(), 4);
        assert_eq!(keys.iter().filter(|k| **k == 42).count(), 10);
    }

    #[tokio::test]
    async fn composing_rejects_bad_input_before_sending() {
        let (s, log) = rig().await;
        let target = || Target {
            track: Ref::Number(1),
            slot: Ref::Number(1),
            name: None,
            mode: None,
        };
        let e = s
            .write_bassline(Parameters(WriteBasslineArgs {
                target: target(),
                key: None,
                progression: "i iv v".into(),
                beats_per_chord: None,
                octave: None,
                style: None,
                rhythm: None,
                step: None,
                velocity: None,
            }))
            .await
            .expect_err("roman without key");
        assert!(e.message.contains("key"));
        assert!(
            s.write_arp(Parameters(WriteArpArgs {
                target: target(),
                key: None,
                progression: "Am Hm".into(),
                beats_per_chord: None,
                octave: None,
                direction: None,
                rate: None,
                octaves: None,
                gate: None,
                velocity: None,
                seed: None,
            }))
            .await
            .is_err()
        );
        assert!(sent(&log, "create_clip").is_empty() && sent(&log, "write_notes").is_empty());
    }

    #[tokio::test]
    async fn song_plan_round_trips_as_json() {
        let (s, log) = rig().await;
        s.set_song_plan(Parameters(SongPlanArgs {
            plan: json!({"key": "A minor", "tempo": 124}),
        }))
        .await
        .expect("set");
        let stored = sent(&log, "set_song_plan")[0]["text"]
            .as_str()
            .expect("text")
            .to_owned();
        assert_eq!(
            serde_json::from_str::<Json>(&stored).expect("json")["tempo"],
            124
        );
        let got = s.get_song_plan().await.expect("get");
        assert!(got.contains("A minor"), "{got}");
    }

    #[tokio::test]
    async fn copy_clip_writes_each_copy_with_its_own_variation() {
        let (s, log) = rig().await;
        let r = s
            .copy_clip(Parameters(CopyClipArgs {
                track: Ref::Name("synth".into()),
                slot: Ref::Name("intro".into()),
                to_track: None,
                to: vec![
                    CopyDest {
                        slot: Ref::Name("verse".into()),
                        ops: None,
                        name: Some("pad 2".into()),
                    },
                    CopyDest {
                        slot: Ref::Number(5),
                        ops: Some(vec![theory::Transform::Transpose { semitones: 12 }]),
                        name: None,
                    },
                ],
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"source_notes\":2"), "{r}");
        let w = sent(&log, "write_notes");
        assert_eq!(w.len(), 2);
        assert_eq!(w[0]["clear"], true);
        let keys = |i: usize| -> Vec<u64> {
            w[i]["notes"]
                .as_array()
                .expect("notes")
                .iter()
                .map(|n| n["key"].as_u64().expect("key"))
                .collect()
        };
        assert_eq!(keys(0), vec![60, 64]);
        assert_eq!(keys(1), vec![72, 76]);
        assert_eq!(sent(&log, "set_clip")[0]["name"], "pad 2");
    }

    #[tokio::test]
    async fn build_song_runs_parts_into_the_section_scenes() {
        let (s, log) = rig().await;
        let r = s
            .build_song(Parameters(BuildSongArgs {
                sections: Some(vec![json!({
                    "name": "Verse",
                    "parts": [
                        {"kind": "chords", "track": "Synth", "progression": "i VI", "key": "A minor"},
                        {"kind": "bass", "track": "Synth", "progression": "i VI", "key": "A minor"},
                        {"kind": "ufo", "track": "Synth"}
                    ]
                })]),
                only: None,
            }))
            .await
            .expect("tool");
        assert!(
            r.contains("\"built\":2") && r.contains("\"failed\":1"),
            "{r}"
        );
        assert_eq!(sent(&log, "write_notes").len(), 2);
        assert!(r.contains("ufo"), "{r}");
    }

    #[tokio::test]
    async fn write_melody_writes_notes_in_the_key() {
        let (s, log) = rig().await;
        let r = s
            .write_melody(Parameters(WriteMelodyArgs {
                target: Target {
                    track: Ref::Name("synth".into()),
                    slot: Ref::Name("verse".into()),
                    name: Some("hook".into()),
                    mode: None,
                },
                key: "A minor".into(),
                progression: "i VI III VII".into(),
                beats_per_chord: None,
                octave: None,
                span: None,
                contour: None,
                density: None,
                rate: None,
                motif_beats: None,
                rhythm: None,
                variation: None,
                velocity: None,
                seed: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"new_clip\":true"), "{r}");
        assert_eq!(sent(&log, "create_clip")[0]["length"], 16);
        let scale = theory::Scale::parse("A minor").expect("key");
        let w = &sent(&log, "write_notes")[0];
        let notes = w["notes"].as_array().expect("notes");
        assert!(notes.len() >= 8);
        assert!(
            notes
                .iter()
                .all(|n| scale.contains(n["key"].as_u64().expect("key") as u8))
        );
    }

    #[tokio::test]
    async fn insert_chain_loads_devices_in_order_and_sets_params() {
        let (s, log) = rig().await;
        let r = s
            .insert_chain(Parameters(InsertChainArgs {
                track: Ref::Name("synth".into()),
                devices: vec![
                    ChainDevice {
                        name: Some("EQ-5".into()),
                        preset: None,
                        params: Vec::new(),
                    },
                    ChainDevice {
                        name: Some("Peak Limiter".into()),
                        preset: None,
                        params: vec![ParamSetting {
                            page: None,
                            param: Ref::Number(1),
                            value: 0.5,
                        }],
                    },
                ],
            }))
            .await
            .expect("tool");
        assert!(!r.contains("error"), "{r}");
        let ins = sent(&log, "insert_device");
        assert_eq!(ins.len(), 2);
        assert_eq!(ins[0]["source"]["name"], "EQ-5");
        assert_eq!(ins[1]["source"]["name"], "Peak Limiter");
        assert_eq!(sent(&log, "set_device_param").len(), 1);
    }

    #[tokio::test]
    async fn master_chain_defaults_to_eq_compressor_limiter() {
        let (s, log) = rig().await;
        s.master_chain(Parameters(MasterChainArgs::default()))
            .await
            .expect("tool");
        let names: Vec<_> = sent(&log, "insert_device")
            .iter()
            .map(|r| r["source"]["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(names, ["EQ-5", "Compressor", "Peak Limiter"]);
    }

    #[tokio::test]
    async fn create_return_makes_an_effect_track_and_sets_the_sends() {
        let (s, log) = rig().await;
        let r = s
            .create_return(Parameters(CreateReturnArgs {
                name: "Reverb".into(),
                devices: vec![ChainDevice {
                    name: Some("Reverb".into()),
                    preset: None,
                    params: Vec::new(),
                }],
                color: None,
                sends: vec![ReturnSend {
                    track: Ref::Name("drums".into()),
                    volume_db: -12.0,
                }],
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"ok\":true"), "{r}");
        assert_eq!(sent(&log, "create_track")[0]["kind"], "effect");
        assert_eq!(sent(&log, "insert_device").len(), 1);
        assert!(
            sent(&log, "set_track")
                .iter()
                .any(|r| r["sends"][0]["volume"].is_number()),
        );
    }

    #[tokio::test]
    async fn drum_kit_effects_append_to_the_pad() {
        let (s, log) = rig().await;
        let mut effects = std::collections::BTreeMap::new();
        effects.insert(
            "snare".to_owned(),
            vec!["Gate".to_owned(), "Reverb".to_owned()],
        );
        s.build_drum_kit(Parameters(DrumKitArgs {
            track: Ref::Name("drums".into()),
            device: None,
            pads: std::collections::BTreeMap::new(),
            effects,
        }))
        .await
        .expect("tool");
        let sent = sent(&log, "drum_pad_insert");
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|r| r["append"] == true && r["key"] == 38));
        assert_eq!(sent[1]["source"]["name"], "Reverb");
    }

    #[tokio::test]
    async fn get_meters_passes_the_window() {
        let (s, log) = rig().await;
        s.get_meters(Parameters(MetersArgs {
            window_ms: Some(50),
        }))
        .await
        .expect("tool");
        assert_eq!(sent(&log, "get_meters")[0]["window_ms"], 50);
    }

    #[test]
    fn bars_and_curves() {
        assert_eq!(beats_per_bar("4/4"), 4.0);
        assert_eq!(beats_per_bar("6/8"), 3.0);
        assert_eq!(beats_per_bar("nonsense"), 4.0);
        assert!(shaped(0.5, "ease_in") < 0.5 && shaped(0.5, "ease_out") > 0.5);
        assert_eq!(shaped(0.5, "linear"), 0.5);
    }

    #[tokio::test]
    async fn record_arrangement_launches_scenes_in_order_between_record_and_stop() {
        let (s, log) = rig().await;
        let r = s
            .record_arrangement(Parameters(RecordArrangementArgs {
                scenes: Some(vec![
                    ArrangementScene {
                        scene: Ref::Name("intro".into()),
                        bars: Some(1),
                    },
                    ArrangementScene {
                        scene: Ref::Name("verse".into()),
                        bars: Some(1),
                    },
                ]),
                start_beat: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"recorded_bars\":2"), "{r}");
        let order: Vec<String> = log
            .lock()
            .expect("log")
            .iter()
            .filter(|q| matches!(q["cmd"].as_str(), Some("transport" | "scene_action")))
            .map(|q| {
                q["action"].as_str().unwrap_or_default().to_owned()
                    + &q["scene"].as_u64().map_or(String::new(), |n| n.to_string())
            })
            .collect();
        assert_eq!(order[..4], ["stop", "stop_all_clips", "record", "launch0"]);
        assert_eq!(order[4], "launch1");
        assert_eq!(order.last().map(String::as_str), Some("stop"));
    }

    #[tokio::test]
    async fn record_automation_sweeps_the_parameter() {
        let (s, log) = rig().await;
        let r = s
            .record_automation(Parameters(RecordAutomationArgs {
                track: Ref::Name("synth".into()),
                device: Ref::Number(1),
                page: None,
                param: Ref::Number(1),
                from: 0.1,
                to: 0.9,
                bars: 1,
                start_beat: None,
                scene: None,
                curve: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"steps\""), "{r}");
        let sets = sent(&log, "set_device_param");
        assert!(sets.len() >= 5);
        let last = sets.last().expect("last")["value"].as_f64().expect("v");
        assert!((last - 0.9).abs() < 1e-9);
        assert!(sent(&log, "set_transport")[0]["automation_write"] == true);
    }

    #[tokio::test]
    async fn transform_clip_needs_a_clip() {
        let (s, _) = rig().await;
        let e = s
            .transform_clip(Parameters(TransformClipArgs {
                track: Ref::Number(1),
                slot: Ref::Number(1),
                ops: vec![theory::Transform::Transpose { semitones: 12 }],
            }))
            .await
            .expect_err("empty slot");
        assert!(e.message.contains("no clip"));
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#ff8000"), Ok(Rgb(255, 128, 0)));
        assert_eq!(parse_color("00ff00"), Ok(Rgb(0, 255, 0)));
        assert_eq!(parse_color("Blue"), Ok(Rgb(0, 153, 217)));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("#gggggg").is_err());
        assert!(parse_color("chartreuse-ish").is_err());
    }

    #[test]
    fn resolutions() {
        // note values, like every other step argument: "1/16" is a sixteenth note
        assert_eq!(parse_step(Some("1/16"), 1.0).ok(), Some(0.25));
        assert!(parse_step(Some("1/0"), 1.0).is_err());
    }

    #[test]
    fn volumes() {
        assert_eq!(volume_of(Some(0.5), None).ok(), Some(Some(0.5)));
        let db0 = volume_of(Some(0.1), Some(0.0)).expect("db").expect("some");
        assert!((db0 - 0.7937).abs() < 1e-3, "dB wins");
        assert!(volume_of(Some(1.5), None).is_err());
        assert!(volume_of(None, Some(f64::NAN)).is_err());
    }

    #[test]
    fn created_track_type_is_checked() {
        assert!(check_created_type(TrackKind::Instrument, Some("Instrument")).is_ok());
        assert!(check_created_type(TrackKind::Instrument, Some("Hybrid")).is_ok());
        assert!(check_created_type(TrackKind::Audio, Some("Audio")).is_ok());
        assert!(check_created_type(TrackKind::Effect, Some("Effect")).is_ok());
        assert!(check_created_type(TrackKind::Instrument, Some("Master")).is_err());
        assert!(check_created_type(TrackKind::Audio, Some("Effect")).is_err());
        assert!(check_created_type(TrackKind::Instrument, None).is_ok());
    }

    #[test]
    fn picking_names() {
        let names = vec![
            "Main".to_owned(),
            "Filter".to_owned(),
            "Filter Env".to_owned(),
        ];
        assert_eq!(pick(&names, "filter", "page").ok(), Some(1));
        assert_eq!(pick(&names, "env", "page").ok(), Some(2));
        assert_eq!(pick(&names, "2", "page").ok(), Some(1));
        assert!(pick(&names, "9", "page").is_err());
        assert!(pick(&names, "x", "page").is_err());
    }

    #[test]
    fn every_tool_has_a_description_and_the_set_is_small() {
        let tools = BridgeServer::tool_router().list_all();
        assert!((20..=60).contains(&tools.len()), "{} tools", tools.len());
        for t in &tools {
            assert!(
                t.description.as_deref().is_some_and(|d| d.len() > 10),
                "{}",
                t.name
            );
        }
    }

    #[tokio::test]
    async fn set_track_resolves_names_and_converts_db() {
        let (s, log) = rig().await;
        s.set_track(Parameters(SetTrackArgs {
            track: Ref::Name("drums".into()),
            name: None,
            color: Some("red".into()),
            volume: None,
            volume_db: Some(-6.0),
            pan: Some(0.25),
            mute: Some(false),
            solo: None,
            arm: None,
            activated: None,
            monitor: Some("Auto".into()),
            sends: vec![SendArg {
                send: Ref::Name("reverb".into()),
                volume: None,
                volume_db: Some(0.0),
            }],
        }))
        .await
        .expect("tool");
        let req = &sent(&log, "set_track")[0];
        assert_eq!(req["track"], json!({"bank": "main", "index": 0}));
        let v = req["volume"].as_f64().expect("volume");
        assert!((music::volume_to_db(v) + 6.0).abs() < 1e-9);
        assert_eq!(req["color"], json!([217, 46, 36]));
        assert_eq!(req["monitor_mode"], "AUTO");
        assert_eq!(req["sends"][0]["index"], 0);
        assert!(req.get("solo").is_none(), "absent fields are not sent");
    }

    #[tokio::test]
    async fn bad_input_is_rejected_before_anything_is_sent() {
        let (s, log) = rig().await;
        let e = s
            .set_track(Parameters(SetTrackArgs {
                track: Ref::Name("guitar".into()),
                name: None,
                color: None,
                volume: None,
                volume_db: None,
                pan: None,
                mute: Some(true),
                solo: None,
                arm: None,
                activated: None,
                monitor: None,
                sends: vec![],
            }))
            .await
            .expect_err("unknown track");
        assert!(
            e.message.contains("Drums"),
            "lists the tracks: {}",
            e.message
        );
        assert!(sent(&log, "set_track").is_empty());
        assert!(
            s.set_transport(Parameters(SetTransportArgs {
                tempo: Some(5.0),
                r#loop: None,
                loop_start: None,
                loop_length: None,
                metronome: None,
                position: None,
                time_signature: None,
                punch_in: None,
                punch_out: None,
                arranger_overdub: None,
                launcher_overdub: None,
                automation_write: None,
                automation_mode: None,
                pre_roll: None,
                launch_quantization: None,
            }))
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn create_track_loads_a_device_and_names_it() {
        let (s, log) = rig().await;
        let r = s
            .create_track(Parameters(CreateTrackArgs {
                kind: TrackKind::Instrument,
                name: Some("Bass".into()),
                color: Some("#0000ff".into()),
                position: None,
                device: Some("Polymer".into()),
                preset: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"warnings\":[]"), "{r}");
        assert_eq!(sent(&log, "create_track")[0]["position"], -1);
        let ins = &sent(&log, "insert_device")[0];
        assert_eq!(ins["track"]["index"], 2);
        assert_eq!(ins["source"]["name"], "Polymer");
        let set = &sent(&log, "set_track")[0];
        assert_eq!(set["name"], "Bass");
    }

    #[tokio::test]
    async fn create_track_reports_a_missing_device_as_a_warning() {
        let (s, _) = rig().await;
        let r = s
            .create_track(Parameters(CreateTrackArgs {
                kind: TrackKind::Instrument,
                name: None,
                color: None,
                position: Some(1),
                device: Some("Nope".into()),
                preset: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("not found"), "{r}");
    }

    #[tokio::test]
    async fn create_clip_writes_notes_on_the_coarsest_grid() {
        let (s, log) = rig().await;
        s.create_clip(Parameters(CreateClipArgs {
            track: Ref::Number(2),
            slot: Ref::Name("verse".into()),
            length_beats: 4,
            name: Some("Riff".into()),
            color: None,
            notes: vec![
                NoteArg {
                    note: NoteRef::Name("C3".into()),
                    start: 0.0,
                    length: 0.5,
                    velocity: None,
                },
                NoteArg {
                    note: NoteRef::Number(67),
                    start: 1.5,
                    length: 0.5,
                    velocity: Some(90),
                },
            ],
        }))
        .await
        .expect("tool");
        assert_eq!(sent(&log, "create_clip")[0]["slot"], 1);
        assert_eq!(sent(&log, "set_clip")[0]["name"], "Riff");
        let w = &sent(&log, "write_notes")[0];
        assert_eq!(w["step"], 0.5);
        assert_eq!(w["clear"], true);
        assert_eq!(
            w["notes"][0],
            json!({"x": 0, "key": 60, "velocity": 100, "duration": 0.5})
        );
        assert_eq!(w["notes"][1]["x"], 3);
    }

    #[tokio::test]
    async fn create_clip_rejects_notes_past_the_end() {
        let (s, log) = rig().await;
        let e = s
            .create_clip(Parameters(CreateClipArgs {
                track: Ref::Number(1),
                slot: Ref::Number(1),
                length_beats: 4,
                name: None,
                color: None,
                notes: vec![NoteArg {
                    note: NoteRef::Number(60),
                    start: 4.0,
                    length: 1.0,
                    velocity: None,
                }],
            }))
            .await
            .expect_err("late note");
        assert!(e.message.contains("4 beats"));
        assert!(sent(&log, "create_clip").is_empty());
    }

    #[tokio::test]
    async fn device_params_resolve_by_page_and_name() {
        let (s, log) = rig().await;
        s.set_device_param(Parameters(SetParamArgs {
            track: Ref::Name("drums".into()),
            device: Ref::Name("drum".into()),
            page: Some(Ref::Name("filter".into())),
            param: Ref::Name("reso".into()),
            value: 0.3,
        }))
        .await
        .expect("tool");
        let req = &sent(&log, "set_device_param")[0];
        assert_eq!(
            (req["page"].clone(), req["param"].clone()),
            (json!(1), json!(7)),
            "the parameter's own index, not its place in the list"
        );
    }

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

    #[tokio::test]
    async fn create_scene_names_the_new_scene() {
        let (s, log) = rig().await;
        let r = s
            .create_scene(Parameters(CreateSceneArgs {
                name: Some("Chorus".into()),
                color: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"scene\":3"), "{r}");
        assert_eq!(sent(&log, "set_scene")[0]["scene"], 2);
    }

    #[test]
    fn project_summary_drops_the_detail() {
        let snap = json!({
            "application": {"project_name": "p"},
            "tracks": [{"index": 0, "name": "Bass", "type": "Instrument", "volume_text": "-6 dB",
                "pan_text": "0", "mute": true, "solo": false, "color": "#fff",
                "devices": [{"name": "Polysynth", "preset": "x", "enabled": true}],
                "clips": [{"slot": 2, "name": "a", "playing": false}],
                "sends": [{"name": "Reverb", "volume": 0.0}, {"name": "Delay", "volume": 0.5, "volume_text": "-3 dB"}]}],
            "effect_tracks": [],
            "master": {"name": "Master"},
            "scenes": [{"index": 0, "name": "Intro", "clip_count": 1, "color": "#000"}]
        });
        let s = summarize_project(&snap);
        let t = &s["tracks"][0];
        assert_eq!(t["devices"], json!(["Polysynth"]));
        assert_eq!(t["clip_slots"], json!([2]));
        assert_eq!(t["mute"], true);
        assert!(t.get("solo").is_none() && t.get("color").is_none());
        assert_eq!(
            t["sends"],
            json!([{"name": "Delay", "volume_text": "-3 dB"}])
        );
        assert!(s["scenes"][0].get("color").is_none());
        assert_eq!(s["application"]["project_name"], "p");
    }

    #[tokio::test]
    async fn status_without_bitwig_says_so() {
        let s = BridgeServer::new(Arc::new(Bridge::new("127.0.0.1", 1)), PathBuf::new());
        let r = s.status().await.expect("status never fails");
        assert!(r.contains("\"connected\":false"), "{r}");
    }
}
