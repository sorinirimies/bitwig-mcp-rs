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
use super::project::{ProjectView, Ref};
use super::protocol::{
    ClipAction, ClipChange, Command, DeviceAction, DeviceSource, GridNote, Insert, Rgb,
    SceneAction, SendLevel, TrackAction, TrackAddr, TrackChange, TrackKind, TransportAction,
    TransportChange,
};
use crate::hardware::{self, Profile};
use crate::music;

type ToolResult = Result<String, McpError>;

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

/// Parses a grid resolution like "1/16" or "0.25" (beats).
fn parse_resolution(s: &str) -> Result<f64, McpError> {
    let v = match s.split_once('/') {
        Some((n, d)) => {
            let n: f64 = n.trim().parse().map_err(|_| bad("bad resolution"))?;
            let d: f64 = d.trim().parse().map_err(|_| bad("bad resolution"))?;
            n / d
        }
        None => s.trim().parse().map_err(|_| bad("bad resolution"))?,
    };
    if !(1.0 / 64.0..=4.0).contains(&v) {
        return Err(bad("resolution must be between 1/64 and 4 beats"));
    }
    Ok(v)
}

// ---------------------------------------------------------------- parameters

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
    /// Grid to read on, e.g. "1/16" (default) or "1/32"; starts are reported on this grid.
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
    /// Notes (numbers or names); several = a chord.
    pub notes: Vec<NoteRef>,
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
    /// Value 0.0-1.0.
    pub value: Option<f64>,
    /// Raw MIDI value instead (0-127, or 0-16383 for 14-bit parameters).
    pub raw: Option<u16>,
    /// Named step instead, for parameters with named values.
    pub named: Option<String>,
    /// MIDI channel 1-16; default: the profile's.
    pub channel: Option<u8>,
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

// ---------------------------------------------------------------- the server

/// The bridge-backed MCP server.
#[derive(Debug, Clone)]
pub struct BridgeServer {
    bridge: Arc<Bridge>,
    profiles_dir: PathBuf,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
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

    async fn project(&self) -> Result<(Json, ProjectView), McpError> {
        let snap = self.call(Command::GetProject).await?;
        let view = ProjectView::from_snapshot(&snap).map_err(err)?;
        Ok((snap, view))
    }

    async fn view(&self) -> Result<ProjectView, McpError> {
        Ok(self.project().await?.1)
    }

    async fn track_of(&self, r: &Ref) -> Result<(ProjectView, TrackAddr), McpError> {
        let v = self.view().await?;
        let t = v.track(r).map_err(bad)?;
        Ok((v, t))
    }

    fn profiles(&self) -> (Vec<Profile>, Vec<String>) {
        hardware::load_dir(&self.profiles_dir)
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
        self.call_long(Command::InsertDevice { track, at, source })
            .await
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

#[tool_router]
impl BridgeServer {
    // ------------------------------------------------------------ reading

    #[tool(
        description = "Connection status: bridge address, extension and protocol version, project name."
    )]
    async fn status(&self) -> ToolResult {
        match self.call(Command::GetProject).await {
            Ok(snap) => out(&json!({
                "connected": true,
                "bridge": self.bridge.addr(),
                "extension": self.bridge.hello(),
                "project": snap["application"]["project_name"],
            })),
            Err(e) => out(&json!({
                "connected": false,
                "bridge": self.bridge.addr(),
                "error": e.message,
            })),
        }
    }

    #[tool(
        description = "The whole project: transport, every track (mixer, sends, devices, clips), effect tracks, master and scenes. Read this first."
    )]
    async fn get_project(&self) -> ToolResult {
        out(&self.project().await?.0)
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
        let track: TrackAddr = serde_json::from_value(created).map_err(err)?;
        let mut notes = Vec::new();
        let source = match (a.device, a.preset) {
            (_, Some(p)) => Some(DeviceSource::Browser {
                name: p,
                content_type: Some("Presets".into()),
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
            let volume = volume_of(s.volume, s.volume_db)?
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
        let scene = self.view().await?.scene(&a.scene).map_err(bad)?;
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
        let scene = self.view().await?.scene(&a.scene).map_err(bad)?;
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
        let step = a
            .resolution
            .as_deref()
            .map(parse_resolution)
            .transpose()?
            .unwrap_or(0.25);
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
                content_type: Some("Presets".into()),
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
        let (view, track) = self.track_of(&a.track).await?;
        let device = view.device(track, &a.device).map_err(bad)?;
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
        if !(0.0..=1.0).contains(&a.value) {
            return Err(bad("value must be 0.0-1.0"));
        }
        let (view, track) = self.track_of(&a.track).await?;
        let device = view.device(track, &a.device).map_err(bad)?;
        let page = self.page_index(track, device, a.page.as_ref()).await?;
        let param = match &a.param {
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
                pick(&names, name, "parameter")?
            }
        };
        out(&self
            .call(Command::SetDeviceParam {
                track,
                device,
                page,
                param,
                value: a.value,
            })
            .await?)
    }

    #[tool(
        description = "Device action: enable, disable, toggle, delete, duplicate, open_window, close_window, next_preset, previous_preset, select."
    )]
    async fn device_action(&self, Parameters(a): Parameters<DeviceActionArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let device = view.device(track, &a.device).map_err(bad)?;
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
        let keys = a
            .notes
            .iter()
            .map(NoteRef::key)
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
        description = "Hardware instrument profiles (from ~/.config/bitwig-mcp-rs/instruments/*.toml plus the built-in 'generic' General MIDI one) and their parameters."
    )]
    async fn list_instruments(&self) -> ToolResult {
        let (profiles, errors) = self.profiles();
        let list: Vec<Json> = profiles
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "channel": p.channel,
                    "params": p.params,
                })
            })
            .collect();
        out(&json!({
            "dir": self.profiles_dir.display().to_string(),
            "instruments": list,
            "errors": errors,
        }))
    }

    #[tool(
        description = "Set a named parameter of an external synth (from its instrument profile) by sending the mapped CC/NRPN through the track's HW Instrument device. Give value (0-1), raw, or named."
    )]
    async fn hardware_set(&self, Parameters(a): Parameters<HardwareSetArgs>) -> ToolResult {
        let (profiles, _) = self.profiles();
        let pname = a.instrument.as_deref().unwrap_or("generic");
        let profile = hardware::find(&profiles, pname).ok_or_else(|| {
            let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
            bad(format!("no instrument profile {pname:?}; have {names:?}"))
        })?;
        let (name, param) = profile.param(&a.param).ok_or_else(|| {
            let names: Vec<&String> = profile.params.keys().collect();
            bad(format!(
                "{} has no parameter {:?}; have {names:?}",
                profile.name, a.param
            ))
        })?;
        let value = match (a.value, a.raw, a.named) {
            (Some(v), None, None) => hardware::Value::Normalized(v),
            (None, Some(r), None) => hardware::Value::Raw(r),
            (None, None, Some(n)) => hardware::Value::Named(n),
            _ => return Err(bad("give exactly one of value, raw, named")),
        };
        let channel = a.channel.unwrap_or(profile.channel);
        let messages = param.messages(channel, &value).map_err(bad)?;
        let (_, track) = self.track_of(&a.track).await?;
        self.call(Command::SendMidi {
            track,
            messages: messages.clone(),
        })
        .await?;
        out(&json!({"instrument": profile.name, "param": name, "sent": messages}))
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
                .map_err(bad)?;
        let (_, track) = self.track_of(&a.track).await?;
        self.call(Command::SendMidi {
            track,
            messages: messages.clone(),
        })
        .await?;
        out(&json!({"sent": messages}))
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
}

#[prompt_router]
impl BridgeServer {
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
                 1. create_track kind instrument named \"{synth}\" with device \"HW Instrument\".\n\
                 2. Tell me to pick the MIDI output port and audio input in that device.\n\
                 3. play_notes a C major chord to test.\n\
                 4. Write an instrument profile TOML for the {synth} from its MIDI implementation \
                 (CC and NRPN numbers) for ~/.config/bitwig-mcp-rs/instruments/, check it with \
                 list_instruments, then try hardware_set on its cutoff.",
                synth = a.synth
            ),
        )]
    }
}

const RESOURCES: [(&str, &str, &str); 3] = [
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
            "bitwig://project" => self.get_project().await?,
            "bitwig://instruments" => self.list_instruments().await?,
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
            "transport": {"playing": false},
            "tracks": [
                {"bank": "main", "index": 0, "name": "Drums",
                 "devices": [{"index": 0, "name": "Drum Machine"}],
                 "sends": [{"index": 0, "name": "Reverb"}]},
                {"bank": "main", "index": 1, "name": "Synth",
                 "devices": [{"index": 0, "name": "HW Instrument"}]}
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
            Some("get_device") => Ok(json!({
                "pages": ["Main", "Filter"],
                "params": [{"name": "Cutoff"}, {"name": "Resonance"}]
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
        assert_eq!(parse_resolution("1/16").ok(), Some(1.0 / 16.0));
        assert_eq!(parse_resolution("0.5").ok(), Some(0.5));
        assert!(parse_resolution("1/128").is_err());
        assert!(parse_resolution("x").is_err());
        assert!(parse_resolution("1/0").is_err());
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
        assert!((20..=40).contains(&tools.len()), "{} tools", tools.len());
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
            (json!(1), json!(1))
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

    #[tokio::test]
    async fn status_without_bitwig_says_so() {
        let s = BridgeServer::new(Arc::new(Bridge::new("127.0.0.1", 1)), PathBuf::new());
        let r = s.status().await.expect("status never fails");
        assert!(r.contains("\"connected\":false"), "{r}");
    }
}
