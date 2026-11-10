//! The MCP tools. Each tool maps to one or more DrivenByMoss OSC commands.
//!
//! Conventions: track, scene, clip-slot, parameter and send indices are **1-based positions in
//! the current bank** (DrivenByMoss exposes a window of `bank_size` items, default 8; scroll it
//! with `scroll_track_bank` / `scroll_scene_bank`). Levels (volume, pan, parameters) are
//! normalized `0.0..=1.0`.

use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ErrorData as McpError, schemars, tool, tool_handler, tool_router};
use rosc::OscType;
use serde::Deserialize;
use serde_json::Value as Json;

use crate::midi::{self, Note};
use crate::osc::client::Bitwig;
use crate::osc::wire::{flag, parse_arg};

type ToolResult = Result<String, McpError>;

fn err(e: impl Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn bad(msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), None)
}

// ---------------------------------------------------------------- parameter types

/// On/off switch; omit `enabled` to toggle.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FlagArgs {
    /// true = on, false = off, omitted = toggle.
    pub enabled: Option<bool>,
}

/// A bank position.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexArgs {
    /// 1-based position in the current bank.
    pub index: u8,
}

/// Track in the bank.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
}

/// Track plus on/off.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackFlagArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// true = on, false = off, omitted = toggle.
    pub enabled: Option<bool>,
}

/// Track plus a normalized level.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackLevelArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// Normalized 0.0-1.0 (pan: 0.5 = center).
    pub value: f64,
}

/// Track plus a name.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackNameArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// The new name.
    pub name: String,
}

/// Track plus an RGB color.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackColorArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// Red 0-255.
    pub r: u8,
    /// Green 0-255.
    pub g: u8,
    /// Blue 0-255.
    pub b: u8,
}

/// Send level of a track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendLevelArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// 1-based send (effect track) position.
    pub send: u8,
    /// Normalized 0.0-1.0.
    pub value: f64,
}

/// Kind of track to add.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Audio track.
    Audio,
    /// Instrument track.
    Instrument,
    /// Effect (return) track.
    Effect,
}

/// Which track to add.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddTrackArgs {
    /// Kind of track.
    pub kind: TrackKind,
}

/// Direction, optionally by a whole page (bank size) instead of one step.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Next / down / forward.
    Next,
    /// Previous / up / back.
    Previous,
}

impl Direction {
    fn sym(self) -> &'static str {
        match self {
            Self::Next => "+",
            Self::Previous => "-",
        }
    }
}

/// Scroll a bank.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScrollArgs {
    /// Scroll direction.
    pub direction: Direction,
    /// Scroll a whole page instead of one item.
    pub page: Option<bool>,
}

/// Direction only.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DirectionArgs {
    /// Direction.
    pub direction: Direction,
}

/// Tempo.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TempoArgs {
    /// Beats per minute, 20-666.
    pub bpm: f64,
}

/// Playback position.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PositionArgs {
    /// Position in beats (quarter notes) from the project start.
    pub beats: f64,
}

/// Time signature.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SignatureArgs {
    /// Like "4/4" or "7/8".
    pub signature: String,
}

/// Preroll bars.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PrerollArgs {
    /// Bars of preroll: 0, 1, 2 or 4.
    pub bars: u8,
}

/// Automation write mode.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
    /// Latch.
    Latch,
    /// Touch.
    Touch,
    /// Write.
    Write,
}

/// Automation write mode.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AutomationModeArgs {
    /// Mode.
    pub mode: AutomationMode,
}

/// Default launch quantization.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QuantizationArgs {
    /// One of: none, 8, 4, 2, 1, 1/2, 1/4, 1/8, 1/16.
    pub quantization: String,
}

/// Window layout.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Arrange view.
    Arrange,
    /// Mix view.
    Mix,
    /// Edit view.
    Edit,
}

/// Window layout.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LayoutArgs {
    /// Layout.
    pub layout: Layout,
}

/// Panel to toggle.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    /// Note editor.
    NoteEditor,
    /// Automation editor.
    AutomationEditor,
    /// Device panel.
    Devices,
    /// Mixer panel.
    Mixer,
    /// Fullscreen window.
    Fullscreen,
}

/// Panel toggle.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PanelArgs {
    /// Panel.
    pub panel: Panel,
}

/// Run a configured DrivenByMoss action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActionArgs {
    /// Action slot 1-20 (configured in the extension's settings).
    pub action: u8,
}

/// Scene name.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SceneNameArgs {
    /// 1-based scene position in the current bank.
    pub scene: u8,
    /// The new name.
    pub name: String,
}

/// Scene in the bank.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SceneArgs {
    /// 1-based scene position in the current bank.
    pub scene: u8,
}

/// Clip slot.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SlotArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// 1-based slot (scene row) position in the current bank.
    pub slot: u8,
}

/// Create an empty clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateClipArgs {
    /// 1-based track position in the current bank.
    pub track: u8,
    /// 1-based slot (scene row) position in the current bank.
    pub slot: u8,
    /// Clip length in beats (quarter notes).
    pub length_beats: f64,
}

/// Name for the cursor clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NameArgs {
    /// The new name.
    pub name: String,
}

/// Device parameter.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ParamArgs {
    /// 1-based parameter position on the current parameter page (1-8).
    pub param: u8,
    /// Normalized 0.0-1.0.
    pub value: f64,
}

/// Device parameter reset.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ParamIndexArgs {
    /// 1-based parameter position on the current parameter page (1-8).
    pub param: u8,
}

/// Layer volume.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LayerLevelArgs {
    /// 1-based layer position.
    pub layer: u8,
    /// Normalized 0.0-1.0 (pan: 0.5 = center).
    pub value: f64,
}

/// Layer on/off.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LayerFlagArgs {
    /// 1-based layer position.
    pub layer: u8,
    /// true = on, false = off, omitted = toggle.
    pub enabled: Option<bool>,
}

/// Layer index.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LayerArgs {
    /// 1-based layer position.
    pub layer: u8,
}

/// What the browser opens for.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    /// Insert a device after the selected one.
    DeviceAfter,
    /// Insert a device before the selected one.
    DeviceBefore,
    /// Browse presets of the selected device.
    Preset,
}

/// Open the browser.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BrowserOpenArgs {
    /// What to browse for.
    pub mode: BrowserMode,
}

/// Move a browser filter column.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BrowserFilterArgs {
    /// Column 1-6: 1 Favorites, 2 Location, 3 File Type, 4 Category, 5 Tags, 6 Creator.
    pub filter: u8,
    /// Direction.
    pub direction: Direction,
}

/// Reset a browser filter column.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BrowserFilterResetArgs {
    /// Column 1-6.
    pub filter: u8,
}

/// Insert a CLAP plugin.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClapArgs {
    /// CLAP plugin id, e.g. "com.jmoss.dsp-lab".
    pub clap_id: String,
}

/// Audition notes.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlayNotesArgs {
    /// MIDI notes (0-127); several notes play as a chord.
    pub notes: Vec<u8>,
    /// Velocity 1-127 (default 100).
    pub velocity: Option<u8>,
    /// How long to hold, in milliseconds (default 500, max 10000).
    pub duration_ms: Option<u64>,
    /// MIDI channel 1-16 (default 1).
    pub channel: Option<u8>,
}

/// MIDI control change.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CcArgs {
    /// Controller number 0-127.
    pub cc: u8,
    /// Value 0-127.
    pub value: u8,
    /// MIDI channel 1-16 (default 1).
    pub channel: Option<u8>,
}

/// Pitch bend.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PitchBendArgs {
    /// 0-127, 64 = no bend.
    pub value: u8,
    /// MIDI channel 1-16 (default 1).
    pub channel: Option<u8>,
}

/// One note of a clip to record.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct NoteSpec {
    /// MIDI note 0-127 (60 = C3 in Bitwig).
    pub note: u8,
    /// Start in beats from the clip start.
    pub start_beats: f64,
    /// Length in beats.
    pub length_beats: f64,
    /// Velocity 1-127 (default 100).
    pub velocity: Option<u8>,
}

/// Record notes into a new clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordClipArgs {
    /// 1-based track position in the current bank (must be an instrument/hybrid track).
    pub track: u8,
    /// 1-based slot (scene row) position in the current bank; should be empty.
    pub slot: u8,
    /// Clip length in beats (quarter notes).
    pub length_beats: f64,
    /// The notes to record.
    pub notes: Vec<NoteSpec>,
    /// MIDI channel 1-16 (default 1).
    pub channel: Option<u8>,
    /// Tempo used for timing; default: the project tempo reported by Bitwig, else 120.
    pub tempo_bpm: Option<f64>,
}

/// Raw OSC message.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OscSendArgs {
    /// OSC address, e.g. "/track/1/volume".
    pub address: String,
    /// Arguments: numbers, strings, or booleans (sent as 1/0).
    #[serde(default)]
    pub args: Vec<Json>,
}

/// Read cached state.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OscGetArgs {
    /// Address prefix, e.g. "/track/1/" (empty = everything, can be large).
    pub prefix: String,
}

// ---------------------------------------------------------------- the server

/// The MCP server: one Bitwig connection plus the tool router.
#[derive(Debug, Clone)]
pub struct OscServer {
    bw: Arc<Bitwig>,
    tool_router: ToolRouter<Self>,
}

impl OscServer {
    /// Wraps a connection.
    #[must_use]
    pub fn new(bw: Arc<Bitwig>) -> Self {
        Self {
            bw,
            tool_router: Self::tool_router(),
        }
    }

    fn idx(&self, what: &str, v: u8) -> Result<u8, McpError> {
        let max = self.bw.bank_size();
        if v == 0 || v > max {
            return Err(bad(format!(
                "{what} {v} is outside the bank window 1..={max}; scroll the bank or raise --bank-size"
            )));
        }
        Ok(v)
    }

    fn idx_in(what: &str, v: u8, max: u8) -> Result<u8, McpError> {
        if v == 0 || v > max {
            return Err(bad(format!("{what} {v} must be 1..={max}")));
        }
        Ok(v)
    }

    fn channel(c: Option<u8>) -> Result<u8, McpError> {
        Self::idx_in("channel", c.unwrap_or(1), 16)
    }

    async fn osc(&self, addr: &str, args: Vec<OscType>) -> Result<(), McpError> {
        self.bw.send(addr, args).await.map_err(err)
    }

    async fn run(&self, addr: &str, args: Vec<OscType>, done: impl Into<String>) -> ToolResult {
        self.osc(addr, args).await?;
        Ok(done.into())
    }

    /// `{0,1,-}` style: explicit state or toggle.
    async fn tri(&self, addr: &str, enabled: Option<bool>, name: &str) -> ToolResult {
        let args = enabled.map(flag).into_iter().collect();
        let what = match enabled {
            Some(true) => "on",
            Some(false) => "off",
            None => "toggled",
        };
        self.run(addr, args, format!("{name}: {what}")).await
    }

    /// `{1,-}` style: only toggles.
    async fn toggle(&self, addr: &str, name: &str) -> ToolResult {
        self.run(addr, vec![OscType::Int(1)], format!("{name}: toggled"))
            .await
    }

    /// Makes sure we have state; asks Bitwig for it once if the cache is empty.
    async fn ensure_state(&self) -> Result<(), McpError> {
        if !self.bw.listening() {
            return Err(err(
                "state readback is off: the listen port could not be bound (is another \
                 bitwig-mcp instance running?)",
            ));
        }
        if self.bw.with_state(|s| s.is_empty()) {
            self.bw.refresh().await.map_err(err)?;
            let got = self
                .bw
                .wait_for(Duration::from_millis(2500), |s| !s.is_empty())
                .await;
            if !got {
                return Err(err(
                    "Bitwig sent no state. Check that the DrivenByMoss 'Open Sound Control' \
                     controller is added in Bitwig with receive port 8000, send host 127.0.0.1 \
                     and send port 9000",
                ));
            }
        }
        Ok(())
    }

    fn json(v: &Json) -> ToolResult {
        serde_json::to_string_pretty(v).map_err(err)
    }

    async fn send_note(&self, channel: u8, note: u8, velocity: u8) -> Result<(), McpError> {
        self.osc(
            &format!("/vkb_midi/{channel}/note/{note}"),
            vec![OscType::Int(i32::from(velocity))],
        )
        .await
    }
}

#[tool_router]
impl OscServer {
    // ------------------------------------------------------------ reading state

    #[tool(
        description = "Everything at a glance: transport, mixer (tracks, clips, master), scenes and the selected device. Values are normalized 0-1."
    )]
    async fn get_overview(&self) -> ToolResult {
        self.ensure_state().await?;
        let (scale, bank) = (self.bw.scale(), self.bw.bank_size());
        Self::json(&self.bw.with_state(|s| s.overview(scale, bank)))
    }

    #[tool(
        description = "Transport and project state: playing, recording, loop, metronome, tempo, position, time signature, layout."
    )]
    async fn get_transport(&self) -> ToolResult {
        self.ensure_state().await?;
        Self::json(&self.bw.with_state(|s| s.transport()))
    }

    #[tool(
        description = "Tracks in the current bank window with volume, pan, mute, solo, arm, sends and clip slots, plus the master track."
    )]
    async fn get_mixer(&self) -> ToolResult {
        self.ensure_state().await?;
        let (scale, bank) = (self.bw.scale(), self.bw.bank_size());
        Self::json(&self.bw.with_state(|s| s.tracks(scale, bank)))
    }

    #[tool(
        description = "The selected (cursor) device: name, bypass, the 8 parameters of the current page with values, parameter pages, sibling devices and layers."
    )]
    async fn get_device(&self) -> ToolResult {
        self.ensure_state().await?;
        let (scale, bank) = (self.bw.scale(), self.bw.bank_size());
        Self::json(&self.bw.with_state(|s| s.device(scale, bank)))
    }

    #[tool(
        description = "The device/preset browser: active, tab, filter columns with items, and the result list."
    )]
    async fn get_browser(&self) -> ToolResult {
        self.ensure_state().await?;
        Self::json(&self.bw.with_state(|s| s.browser()))
    }

    #[tool(
        description = "Raw cached OSC values whose address starts with a prefix, e.g. '/track/1/' or '/device/param/'. Raw values are 0..MAX_VALUE integers."
    )]
    async fn osc_get(&self, Parameters(a): Parameters<OscGetArgs>) -> ToolResult {
        self.ensure_state().await?;
        let v = self.bw.with_state(|s| {
            let map: serde_json::Map<String, Json> = s
                .with_prefix(&a.prefix)
                .into_iter()
                .take(2000)
                .map(|(k, v)| (k.to_owned(), serde_json::to_value(v).unwrap_or(Json::Null)))
                .collect();
            Json::Object(map)
        });
        Self::json(&v)
    }

    #[tool(
        description = "Ask Bitwig to resend its entire state (use if the overview looks stale)."
    )]
    async fn refresh(&self) -> ToolResult {
        self.run("/refresh", vec![], "refresh requested").await
    }

    // ------------------------------------------------------------ project

    #[tool(description = "Undo the last action.")]
    async fn undo(&self) -> ToolResult {
        self.run("/undo", vec![], "undone").await
    }

    #[tool(description = "Redo the last undone action.")]
    async fn redo(&self) -> ToolResult {
        self.run("/redo", vec![], "redone").await
    }

    #[tool(description = "Save the current project.")]
    async fn save_project(&self) -> ToolResult {
        self.run("/project/save", vec![], "project saved").await
    }

    #[tool(description = "Switch the audio engine on/off (omit enabled to toggle).")]
    async fn audio_engine(&self, Parameters(a): Parameters<FlagArgs>) -> ToolResult {
        self.tri("/project/engine", a.enabled, "audio engine").await
    }

    #[tool(description = "Run one of the 20 actions configured in the DrivenByMoss settings.")]
    async fn run_action(&self, Parameters(a): Parameters<ActionArgs>) -> ToolResult {
        let n = Self::idx_in("action", a.action, 20)?;
        self.run(
            "/action",
            vec![OscType::Int(i32::from(n))],
            format!("action {n} executed"),
        )
        .await
    }

    #[tool(description = "Switch the window layout: arrange, mix or edit.")]
    async fn set_layout(&self, Parameters(a): Parameters<LayoutArgs>) -> ToolResult {
        let l = match a.layout {
            Layout::Arrange => "arrange",
            Layout::Mix => "mix",
            Layout::Edit => "edit",
        };
        self.run(&format!("/layout/{l}"), vec![], format!("layout: {l}"))
            .await
    }

    #[tool(
        description = "Toggle a panel: note_editor, automation_editor, devices, mixer or fullscreen."
    )]
    async fn toggle_panel(&self, Parameters(a): Parameters<PanelArgs>) -> ToolResult {
        let p = match a.panel {
            Panel::NoteEditor => "noteEditor",
            Panel::AutomationEditor => "automationEditor",
            Panel::Devices => "devices",
            Panel::Mixer => "mixer",
            Panel::Fullscreen => "fullscreen",
        };
        self.run(&format!("/panel/{p}"), vec![], format!("panel {p} toggled"))
            .await
    }

    // ------------------------------------------------------------ transport

    #[tool(
        description = "Start or stop playback. enabled=true plays, false stops, omitted toggles."
    )]
    async fn play(&self, Parameters(a): Parameters<FlagArgs>) -> ToolResult {
        self.tri("/play", a.enabled, "playback").await
    }

    #[tool(description = "Stop playback.")]
    async fn stop(&self) -> ToolResult {
        self.run("/stop", vec![], "stopped").await
    }

    #[tool(description = "Restart playback from the beginning of the timeline.")]
    async fn restart(&self) -> ToolResult {
        self.toggle("/restart", "restart").await
    }

    #[tool(description = "Start or stop arranger recording (toggle).")]
    async fn record(&self) -> ToolResult {
        self.run("/record", vec![], "record toggled").await
    }

    #[tool(description = "Set the tempo in BPM (20-666, decimals allowed).")]
    async fn set_tempo(&self, Parameters(a): Parameters<TempoArgs>) -> ToolResult {
        if !(20.0..=666.0).contains(&a.bpm) {
            return Err(bad(format!("tempo {} is outside 20..=666", a.bpm)));
        }
        #[allow(clippy::cast_possible_truncation)]
        let bpm = a.bpm as f32;
        self.run(
            "/tempo/raw",
            vec![OscType::Float(bpm)],
            format!("tempo: {bpm} bpm"),
        )
        .await
    }

    #[tool(description = "Tap the tempo once.")]
    async fn tap_tempo(&self) -> ToolResult {
        self.toggle("/tempo/tap", "tap tempo").await
    }

    #[tool(description = "Turn the loop (repeat) on/off; omit enabled to toggle.")]
    async fn set_loop(&self, Parameters(a): Parameters<FlagArgs>) -> ToolResult {
        self.tri("/repeat", a.enabled, "loop").await
    }

    #[tool(description = "Turn the metronome on/off; omit enabled to toggle.")]
    async fn set_metronome(&self, Parameters(a): Parameters<FlagArgs>) -> ToolResult {
        self.tri("/click", a.enabled, "metronome").await
    }

    #[tool(description = "Set the preroll before recording to 0, 1, 2 or 4 bars.")]
    async fn set_preroll(&self, Parameters(a): Parameters<PrerollArgs>) -> ToolResult {
        if !matches!(a.bars, 0 | 1 | 2 | 4) {
            return Err(bad("preroll must be 0, 1, 2 or 4 bars"));
        }
        self.run(
            "/preroll",
            vec![OscType::Int(i32::from(a.bars))],
            format!("preroll: {} bars", a.bars),
        )
        .await
    }

    #[tool(description = "Punch-in on/off; omit enabled to toggle.")]
    async fn set_punch_in(&self, Parameters(a): Parameters<FlagArgs>) -> ToolResult {
        self.tri("/punchIn", a.enabled, "punch in").await
    }

    #[tool(description = "Punch-out on/off; omit enabled to toggle.")]
    async fn set_punch_out(&self, Parameters(a): Parameters<FlagArgs>) -> ToolResult {
        self.tri("/punchOut", a.enabled, "punch out").await
    }

    #[tool(description = "Toggle arranger overdub.")]
    async fn toggle_arranger_overdub(&self) -> ToolResult {
        self.toggle("/overdub", "arranger overdub").await
    }

    #[tool(description = "Toggle clip launcher overdub.")]
    async fn toggle_launcher_overdub(&self) -> ToolResult {
        self.toggle("/overdub/launcher", "launcher overdub").await
    }

    #[tool(description = "Toggle arranger automation write.")]
    async fn toggle_automation_write(&self) -> ToolResult {
        self.toggle("/autowrite", "automation write").await
    }

    #[tool(description = "Set the automation write mode: latch, touch or write.")]
    async fn set_automation_mode(
        &self,
        Parameters(a): Parameters<AutomationModeArgs>,
    ) -> ToolResult {
        let m = match a.mode {
            AutomationMode::Latch => "latch",
            AutomationMode::Touch => "touch",
            AutomationMode::Write => "write",
        };
        self.run(
            "/automationWriteMode",
            vec![OscType::String(m.into())],
            format!("automation mode: {m}"),
        )
        .await
    }

    #[tool(description = "Move the playback position to a position in beats (quarter notes).")]
    async fn set_position(&self, Parameters(a): Parameters<PositionArgs>) -> ToolResult {
        if !a.beats.is_finite() || a.beats < 0.0 {
            return Err(bad("beats must be >= 0"));
        }
        #[allow(clippy::cast_possible_truncation)]
        let b = a.beats as f32;
        self.run(
            "/time",
            vec![OscType::Float(b)],
            format!("position: {b} beats"),
        )
        .await
    }

    #[tool(description = "Set the global time signature, e.g. '3/4'.")]
    async fn set_time_signature(&self, Parameters(a): Parameters<SignatureArgs>) -> ToolResult {
        let ok = a.signature.split_once('/').is_some_and(|(n, d)| {
            n.trim().parse::<u8>().is_ok_and(|n| n > 0)
                && d.trim().parse::<u8>().is_ok_and(|d| d > 0)
        });
        if !ok {
            return Err(bad("signature must look like 4/4"));
        }
        let sig = a.signature.replace(' ', "");
        self.run(
            "/time/signature",
            vec![OscType::String(sig.clone())],
            format!("time signature: {sig}"),
        )
        .await
    }

    #[tool(description = "Quantize the cursor clip's notes.")]
    async fn quantize_clip(&self) -> ToolResult {
        self.run("/clip/quantize", vec![], "clip quantized").await
    }

    #[tool(
        description = "Set the default clip launch quantization: none, 8, 4, 2, 1, 1/2, 1/4, 1/8 or 1/16."
    )]
    async fn set_launch_quantization(
        &self,
        Parameters(a): Parameters<QuantizationArgs>,
    ) -> ToolResult {
        const OK: [&str; 9] = ["none", "8", "4", "2", "1", "1/2", "1/4", "1/8", "1/16"];
        if !OK.contains(&a.quantization.as_str()) {
            return Err(bad(format!("quantization must be one of {OK:?}")));
        }
        self.run(
            "/launcher/defaultQuantization",
            vec![OscType::String(a.quantization.clone())],
            format!("launch quantization: {}", a.quantization),
        )
        .await
    }

    // ------------------------------------------------------------ tracks

    #[tool(description = "Select a track by its bank position.")]
    async fn select_track(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/select"),
            vec![OscType::Int(1)],
            format!("track {t} selected"),
        )
        .await
    }

    #[tool(description = "Select the next or previous track (moves the bank when needed).")]
    async fn select_adjacent_track(&self, Parameters(a): Parameters<DirectionArgs>) -> ToolResult {
        self.run(
            &format!("/track/{}", a.direction.sym()),
            vec![],
            "track selection moved",
        )
        .await
    }

    #[tool(description = "Scroll the track bank window by one track or one page.")]
    async fn scroll_track_bank(&self, Parameters(a): Parameters<ScrollArgs>) -> ToolResult {
        let page = if a.page.unwrap_or(false) { "page/" } else { "" };
        self.run(
            &format!("/track/bank/{page}{}", a.direction.sym()),
            vec![],
            "track bank scrolled",
        )
        .await
    }

    #[tool(
        description = "Switch the track bank between audio/instrument tracks and effect tracks."
    )]
    async fn toggle_effect_bank(&self) -> ToolResult {
        self.run("/track/toggleBank", vec![], "track bank toggled")
            .await
    }

    #[tool(description = "Set a track's volume, normalized 0.0-1.0 (about 0.79 is 0 dB).")]
    async fn set_track_volume(&self, Parameters(a): Parameters<TrackLevelArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            &format!("/track/{t}/volume"),
            vec![OscType::Int(w)],
            format!("track {t} volume: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Set a track's pan, normalized 0.0 (left) - 1.0 (right), 0.5 = center.")]
    async fn set_track_pan(&self, Parameters(a): Parameters<TrackLevelArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            &format!("/track/{t}/pan"),
            vec![OscType::Int(w)],
            format!("track {t} pan: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Mute/unmute a track; omit enabled to toggle.")]
    async fn set_track_mute(&self, Parameters(a): Parameters<TrackFlagArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.tri(
            &format!("/track/{t}/mute"),
            a.enabled,
            &format!("track {t} mute"),
        )
        .await
    }

    #[tool(description = "Solo/unsolo a track; omit enabled to toggle.")]
    async fn set_track_solo(&self, Parameters(a): Parameters<TrackFlagArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.tri(
            &format!("/track/{t}/solo"),
            a.enabled,
            &format!("track {t} solo"),
        )
        .await
    }

    #[tool(description = "Arm/disarm a track for recording; omit enabled to toggle.")]
    async fn set_track_arm(&self, Parameters(a): Parameters<TrackFlagArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.tri(
            &format!("/track/{t}/recarm"),
            a.enabled,
            &format!("track {t} record arm"),
        )
        .await
    }

    #[tool(description = "Toggle input monitoring on a track.")]
    async fn toggle_track_monitor(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.toggle(
            &format!("/track/{t}/monitor"),
            &format!("track {t} monitor"),
        )
        .await
    }

    #[tool(description = "Activate/deactivate a track; omit enabled to toggle.")]
    async fn set_track_activated(&self, Parameters(a): Parameters<TrackFlagArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.tri(
            &format!("/track/{t}/activated"),
            a.enabled,
            &format!("track {t} activation"),
        )
        .await
    }

    #[tool(description = "Rename a track.")]
    async fn rename_track(&self, Parameters(a): Parameters<TrackNameArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/name"),
            vec![OscType::String(a.name.clone())],
            format!("track {t} renamed to {:?}", a.name),
        )
        .await
    }

    #[tool(description = "Set a track's color (RGB 0-255).")]
    async fn set_track_color(&self, Parameters(a): Parameters<TrackColorArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        let c = format!("rgb({},{},{})", a.r, a.g, a.b);
        self.run(
            &format!("/track/{t}/color"),
            vec![OscType::String(c.clone())],
            format!("track {t} color: {c}"),
        )
        .await
    }

    #[tool(description = "Set the send level of a track to an effect track, normalized 0.0-1.0.")]
    async fn set_track_send(&self, Parameters(a): Parameters<SendLevelArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        let s = self.idx("send", a.send)?;
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            &format!("/track/{t}/send/{s}/volume"),
            vec![OscType::Int(w)],
            format!("track {t} send {s}: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Add a new track at the end: audio, instrument or effect.")]
    async fn add_track(&self, Parameters(a): Parameters<AddTrackArgs>) -> ToolResult {
        let k = match a.kind {
            TrackKind::Audio => "audio",
            TrackKind::Instrument => "instrument",
            TrackKind::Effect => "effect",
        };
        self.run(
            &format!("/track/add/{k}"),
            vec![],
            format!("{k} track added"),
        )
        .await
    }

    #[tool(description = "Duplicate a track.")]
    async fn duplicate_track(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/duplicate"),
            vec![],
            format!("track {t} duplicated"),
        )
        .await
    }

    #[tool(description = "DELETE a track and its content. Undo with the undo tool.")]
    async fn remove_track(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/remove"),
            vec![],
            format!("track {t} removed"),
        )
        .await
    }

    #[tool(description = "Enter a group track (show its children in the bank).")]
    async fn enter_group(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/enter"),
            vec![],
            format!("entered group {t}"),
        )
        .await
    }

    #[tool(description = "Leave the current group: select the parent track.")]
    async fn exit_group(&self) -> ToolResult {
        self.run("/track/parent", vec![], "selected parent group")
            .await
    }

    #[tool(description = "Stop the playing clips on a track.")]
    async fn stop_track_clips(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/clip/stop"),
            vec![],
            format!("track {t} clips stopped"),
        )
        .await
    }

    #[tool(description = "Set the master track volume, normalized 0.0-1.0.")]
    async fn set_master_volume(&self, Parameters(a): Parameters<TrackLevelArgs>) -> ToolResult {
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            "/master/volume",
            vec![OscType::Int(w)],
            format!("master volume: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Set the master track pan, normalized 0.0-1.0 (0.5 = center).")]
    async fn set_master_pan(&self, Parameters(a): Parameters<TrackLevelArgs>) -> ToolResult {
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            "/master/pan",
            vec![OscType::Int(w)],
            format!("master pan: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    // ------------------------------------------------------------ scenes and clips

    #[tool(
        description = "Scroll the scene bank window (and with it the clip slot rows) by one scene or one page."
    )]
    async fn scroll_scene_bank(&self, Parameters(a): Parameters<ScrollArgs>) -> ToolResult {
        let page = if a.page.unwrap_or(false) { "bank/" } else { "" };
        self.run(
            &format!("/scene/{page}{}", a.direction.sym()),
            vec![OscType::Int(1)],
            "scene bank scrolled",
        )
        .await
    }

    #[tool(description = "Launch a scene (starts every clip in that row).")]
    async fn launch_scene(&self, Parameters(a): Parameters<SceneArgs>) -> ToolResult {
        let s = self.idx("scene", a.scene)?;
        self.run(
            &format!("/scene/{s}/launch"),
            vec![OscType::Int(1)],
            format!("scene {s} launched"),
        )
        .await
    }

    #[tool(description = "Select a scene.")]
    async fn select_scene(&self, Parameters(a): Parameters<SceneArgs>) -> ToolResult {
        let s = self.idx("scene", a.scene)?;
        self.run(
            &format!("/scene/{s}/select"),
            vec![OscType::Int(1)],
            format!("scene {s} selected"),
        )
        .await
    }

    #[tool(description = "Add an empty scene at the end of the scene list.")]
    async fn add_scene(&self) -> ToolResult {
        self.toggle("/scene/add", "add scene").await
    }

    #[tool(description = "Create a new scene from all currently playing clips.")]
    async fn create_scene_from_playing(&self) -> ToolResult {
        self.toggle("/scene/create", "create scene from playing clips")
            .await
    }

    #[tool(description = "Rename a scene.")]
    async fn rename_scene(&self, Parameters(a): Parameters<SceneNameArgs>) -> ToolResult {
        let s = self.idx("scene", a.scene)?;
        self.run(
            &format!("/scene/{s}/name"),
            vec![OscType::String(a.name.clone())],
            format!("scene {s} renamed to {:?}", a.name),
        )
        .await
    }

    #[tool(description = "Duplicate a scene.")]
    async fn duplicate_scene(&self, Parameters(a): Parameters<SceneArgs>) -> ToolResult {
        let s = self.idx("scene", a.scene)?;
        self.run(
            &format!("/scene/{s}/duplicate"),
            vec![],
            format!("scene {s} duplicated"),
        )
        .await
    }

    #[tool(description = "DELETE a scene. Undo with the undo tool.")]
    async fn remove_scene(&self, Parameters(a): Parameters<SceneArgs>) -> ToolResult {
        let s = self.idx("scene", a.scene)?;
        self.run(
            &format!("/scene/{s}/remove"),
            vec![],
            format!("scene {s} removed"),
        )
        .await
    }

    #[tool(description = "Launch the clip in a slot (track, slot).")]
    async fn launch_clip(&self, Parameters(a): Parameters<SlotArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        self.run(
            &format!("/track/{t}/clip/{s}/launch"),
            vec![OscType::Int(1)],
            format!("clip {t}/{s} launched"),
        )
        .await
    }

    #[tool(
        description = "Select the clip slot (track, slot). The cursor clip tools then act on it."
    )]
    async fn select_clip(&self, Parameters(a): Parameters<SlotArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        self.run(
            &format!("/track/{t}/clip/{s}/select"),
            vec![],
            format!("clip {t}/{s} selected"),
        )
        .await
    }

    #[tool(
        description = "Create an empty clip of a given length in a slot, activate overdub and start it."
    )]
    async fn create_clip(&self, Parameters(a): Parameters<CreateClipArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        if !a.length_beats.is_finite() || a.length_beats <= 0.0 {
            return Err(bad("length_beats must be > 0"));
        }
        #[allow(clippy::cast_possible_truncation)]
        let len = a.length_beats as f32;
        self.run(
            &format!("/track/{t}/clip/{s}/create"),
            vec![OscType::Float(len)],
            format!("clip {t}/{s} created ({len} beats)"),
        )
        .await
    }

    #[tool(description = "Start recording a clip in a slot.")]
    async fn record_clip(&self, Parameters(a): Parameters<SlotArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        self.run(
            &format!("/track/{t}/clip/{s}/record"),
            vec![],
            format!("recording clip {t}/{s}"),
        )
        .await
    }

    #[tool(description = "Duplicate the clip in a slot.")]
    async fn duplicate_clip(&self, Parameters(a): Parameters<SlotArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        self.run(
            &format!("/track/{t}/clip/{s}/duplicate"),
            vec![],
            format!("clip {t}/{s} duplicated"),
        )
        .await
    }

    #[tool(description = "DELETE the clip in a slot. Undo with the undo tool.")]
    async fn remove_clip(&self, Parameters(a): Parameters<SlotArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        self.run(
            &format!("/track/{t}/clip/{s}/remove"),
            vec![],
            format!("clip {t}/{s} removed"),
        )
        .await
    }

    #[tool(description = "Rename the cursor (selected) clip.")]
    async fn rename_clip(&self, Parameters(a): Parameters<NameArgs>) -> ToolResult {
        self.run(
            "/clip/name",
            vec![OscType::String(a.name.clone())],
            format!("clip renamed to {:?}", a.name),
        )
        .await
    }

    #[tool(description = "Stop every playing clip.")]
    async fn stop_all_clips(&self) -> ToolResult {
        self.run("/clip/stopall", vec![], "all clips stopped").await
    }

    #[tool(description = "Switch playback of a track back to the arrangement.")]
    async fn return_to_arrangement(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let t = self.idx("track", a.track)?;
        self.run(
            &format!("/track/{t}/clip/returntoarrangement"),
            vec![],
            format!("track {t} back to arrangement"),
        )
        .await
    }

    // ------------------------------------------------------------ devices

    #[tool(description = "Select the next or previous device in the chain.")]
    async fn select_adjacent_device(&self, Parameters(a): Parameters<DirectionArgs>) -> ToolResult {
        self.run(
            &format!("/device/{}", a.direction.sym()),
            vec![],
            "device selection moved",
        )
        .await
    }

    #[tool(
        description = "Select a device among the siblings in the current chain (see get_device siblings)."
    )]
    async fn select_device_sibling(&self, Parameters(a): Parameters<IndexArgs>) -> ToolResult {
        let i = self.idx("sibling", a.index)?;
        self.run(
            &format!("/device/sibling/{i}/selected"),
            vec![OscType::Int(1)],
            format!("sibling {i} selected"),
        )
        .await
    }

    #[tool(
        description = "Set one of the 8 parameters on the current parameter page of the selected device, normalized 0.0-1.0."
    )]
    async fn set_device_param(&self, Parameters(a): Parameters<ParamArgs>) -> ToolResult {
        let p = self.idx("param", a.param)?;
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            &format!("/device/param/{p}/value"),
            vec![OscType::Int(w)],
            format!("device param {p}: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Reset a device parameter to its default.")]
    async fn reset_device_param(&self, Parameters(a): Parameters<ParamIndexArgs>) -> ToolResult {
        let p = self.idx("param", a.param)?;
        self.run(
            &format!("/device/param/{p}/reset"),
            vec![],
            format!("device param {p} reset"),
        )
        .await
    }

    #[tool(description = "Switch to the next or previous parameter page of the selected device.")]
    async fn device_param_page(&self, Parameters(a): Parameters<DirectionArgs>) -> ToolResult {
        self.run(
            &format!("/device/param/{}", a.direction.sym()),
            vec![OscType::Int(1)],
            "parameter page moved",
        )
        .await
    }

    #[tool(description = "Select a parameter page of the selected device by position.")]
    async fn select_device_page(&self, Parameters(a): Parameters<IndexArgs>) -> ToolResult {
        let i = self.idx("page", a.index)?;
        self.run(
            "/device/page/selected",
            vec![OscType::Int(i32::from(i))],
            format!("page {i} selected"),
        )
        .await
    }

    #[tool(description = "Bypass/enable the selected device (toggle).")]
    async fn toggle_device_bypass(&self) -> ToolResult {
        self.run("/device/bypass", vec![], "device bypass toggled")
            .await
    }

    #[tool(description = "Open/close the plugin window of the selected device.")]
    async fn toggle_device_window(&self) -> ToolResult {
        self.run("/device/window", vec![], "device window toggled")
            .await
    }

    #[tool(description = "Duplicate the selected device.")]
    async fn duplicate_device(&self) -> ToolResult {
        self.run("/device/duplicate", vec![], "device duplicated")
            .await
    }

    #[tool(description = "DELETE the selected device. Undo with the undo tool.")]
    async fn remove_device(&self) -> ToolResult {
        self.run("/device/remove", vec![], "device removed").await
    }

    #[tool(
        description = "Select a layer of the selected container device (e.g. Instrument Layer, Drum Machine)."
    )]
    async fn select_device_layer(&self, Parameters(a): Parameters<LayerArgs>) -> ToolResult {
        let l = self.idx("layer", a.layer)?;
        self.run(
            &format!("/device/layer/{l}/selected"),
            vec![],
            format!("layer {l} selected"),
        )
        .await
    }

    #[tool(description = "Enter the device chain of a layer.")]
    async fn enter_device_layer(&self, Parameters(a): Parameters<LayerArgs>) -> ToolResult {
        let l = self.idx("layer", a.layer)?;
        self.run(
            &format!("/device/layer/{l}/enter"),
            vec![],
            format!("entered layer {l}"),
        )
        .await
    }

    #[tool(description = "Leave the current layer's device chain (go to the parent).")]
    async fn exit_device_layer(&self) -> ToolResult {
        self.run("/device/layer/parent", vec![], "left layer").await
    }

    #[tool(description = "Set a device layer's volume, normalized 0.0-1.0.")]
    async fn set_layer_volume(&self, Parameters(a): Parameters<LayerLevelArgs>) -> ToolResult {
        let l = self.idx("layer", a.layer)?;
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            &format!("/device/layer/{l}/volume"),
            vec![OscType::Int(w)],
            format!("layer {l} volume: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Set a device layer's pan, normalized 0.0-1.0 (0.5 = center).")]
    async fn set_layer_pan(&self, Parameters(a): Parameters<LayerLevelArgs>) -> ToolResult {
        let l = self.idx("layer", a.layer)?;
        let w = self.bw.scale().to_wire(a.value);
        self.run(
            &format!("/device/layer/{l}/pan"),
            vec![OscType::Int(w)],
            format!("layer {l} pan: {:.3}", a.value.clamp(0.0, 1.0)),
        )
        .await
    }

    #[tool(description = "Mute/unmute a device layer; omit enabled to toggle.")]
    async fn set_layer_mute(&self, Parameters(a): Parameters<LayerFlagArgs>) -> ToolResult {
        let l = self.idx("layer", a.layer)?;
        self.tri(
            &format!("/device/layer/{l}/mute"),
            a.enabled,
            &format!("layer {l} mute"),
        )
        .await
    }

    #[tool(description = "Solo/unsolo a device layer; omit enabled to toggle.")]
    async fn set_layer_solo(&self, Parameters(a): Parameters<LayerFlagArgs>) -> ToolResult {
        let l = self.idx("layer", a.layer)?;
        self.tri(
            &format!("/device/layer/{l}/solo"),
            a.enabled,
            &format!("layer {l} solo"),
        )
        .await
    }

    // ------------------------------------------------------------ browser

    #[tool(
        description = "Open the browser to insert a device after/before the selected one, or to browse its presets. Then use get_browser, browser_* navigation, and browser_commit."
    )]
    async fn browser_open(&self, Parameters(a): Parameters<BrowserOpenArgs>) -> ToolResult {
        let addr = match a.mode {
            BrowserMode::DeviceAfter => "/browser/device/after",
            BrowserMode::DeviceBefore => "/browser/device/before",
            BrowserMode::Preset => "/browser/preset",
        };
        self.run(addr, vec![], "browser opened").await
    }

    #[tool(description = "Commit the current browser selection (loads the device or preset).")]
    async fn browser_commit(&self) -> ToolResult {
        self.run("/browser/commit", vec![], "browser selection committed")
            .await
    }

    #[tool(description = "Close the browser without loading anything.")]
    async fn browser_cancel(&self) -> ToolResult {
        self.run("/browser/cancel", vec![], "browser cancelled")
            .await
    }

    #[tool(description = "Next/previous browser tab (Devices, Presets, Multisamples, ...).")]
    async fn browser_tab(&self, Parameters(a): Parameters<DirectionArgs>) -> ToolResult {
        self.run(
            &format!("/browser/tab/{}", a.direction.sym()),
            vec![],
            "browser tab moved",
        )
        .await
    }

    #[tool(
        description = "Move the selection in a browser filter column (1 Favorites, 2 Location, 3 File Type, 4 Category, 5 Tags, 6 Creator)."
    )]
    async fn browser_filter(&self, Parameters(a): Parameters<BrowserFilterArgs>) -> ToolResult {
        let f = Self::idx_in("filter", a.filter, 6)?;
        self.run(
            &format!("/browser/filter/{f}/{}", a.direction.sym()),
            vec![],
            format!("filter {f} moved"),
        )
        .await
    }

    #[tool(description = "Reset a browser filter column.")]
    async fn browser_filter_reset(
        &self,
        Parameters(a): Parameters<BrowserFilterResetArgs>,
    ) -> ToolResult {
        let f = Self::idx_in("filter", a.filter, 6)?;
        self.run(
            &format!("/browser/filter/{f}/reset"),
            vec![],
            format!("filter {f} reset"),
        )
        .await
    }

    #[tool(description = "Select the next/previous browser result.")]
    async fn browser_result(&self, Parameters(a): Parameters<DirectionArgs>) -> ToolResult {
        self.run(
            &format!("/browser/result/{}", a.direction.sym()),
            vec![],
            "browser result moved",
        )
        .await
    }

    #[tool(description = "Insert a CLAP plugin by id after the selected device.")]
    async fn insert_clap(&self, Parameters(a): Parameters<ClapArgs>) -> ToolResult {
        if a.clap_id.trim().is_empty() {
            return Err(bad("clap_id is empty"));
        }
        self.run(
            "/browser/insertClap",
            vec![OscType::String(a.clap_id.clone())],
            format!("inserting CLAP {}", a.clap_id),
        )
        .await
    }

    // ------------------------------------------------------------ MIDI

    #[tool(
        description = "Play notes on the selected/armed track for a while (audition, or capture while recording). Several notes = chord."
    )]
    async fn play_notes(&self, Parameters(a): Parameters<PlayNotesArgs>) -> ToolResult {
        let ch = Self::channel(a.channel)?;
        let vel = a.velocity.unwrap_or(100);
        if a.notes.is_empty() {
            return Err(bad("notes is empty"));
        }
        if a.notes.iter().any(|n| *n > 127) || vel == 0 || vel > 127 {
            return Err(bad("notes must be 0..=127 and velocity 1..=127"));
        }
        let ms = a.duration_ms.unwrap_or(500).min(10_000);
        for n in &a.notes {
            self.send_note(ch, *n, vel).await?;
        }
        tokio::time::sleep(Duration::from_millis(ms)).await;
        for n in &a.notes {
            self.send_note(ch, *n, 0).await?;
        }
        Ok(format!("played {:?} for {ms} ms on channel {ch}", a.notes))
    }

    #[tool(description = "Send a MIDI control change to the selected/armed track.")]
    async fn send_cc(&self, Parameters(a): Parameters<CcArgs>) -> ToolResult {
        let ch = Self::channel(a.channel)?;
        if a.cc > 127 || a.value > 127 {
            return Err(bad("cc and value must be 0..=127"));
        }
        self.run(
            &format!("/vkb_midi/{ch}/cc/{}", a.cc),
            vec![OscType::Int(i32::from(a.value))],
            format!("cc {} = {}", a.cc, a.value),
        )
        .await
    }

    #[tool(description = "Send MIDI pitch bend (0-127, 64 = center) to the selected/armed track.")]
    async fn send_pitch_bend(&self, Parameters(a): Parameters<PitchBendArgs>) -> ToolResult {
        let ch = Self::channel(a.channel)?;
        if a.value > 127 {
            return Err(bad("value must be 0..=127"));
        }
        self.run(
            &format!("/vkb_midi/{ch}/pitchbend"),
            vec![OscType::Int(i32::from(a.value))],
            format!("pitch bend {}", a.value),
        )
        .await
    }

    #[tool(
        description = "Create a clip in a slot and record a list of notes into it live. Selects and arms the track, creates the clip (overdub on), then plays the notes in real time at the project tempo. Timing is wall-clock (a few ms of jitter): set_launch_quantization 'none' first for a tight start, and quantize_clip after. The clip must have free slot space; notes beyond length_beats are cut off by the loop."
    )]
    async fn record_midi_clip(&self, Parameters(a): Parameters<RecordClipArgs>) -> ToolResult {
        let (t, s) = (self.idx("track", a.track)?, self.idx("slot", a.slot)?);
        let ch = Self::channel(a.channel)?;
        if !a.length_beats.is_finite() || a.length_beats <= 0.0 {
            return Err(bad("length_beats must be > 0"));
        }
        let tempo = a
            .tempo_bpm
            .or_else(|| {
                self.bw.with_state(|st| match st.get("/tempo/raw") {
                    Some(crate::osc::state::Value::Float(f)) => Some(*f),
                    Some(crate::osc::state::Value::Int(i)) => Some(*i as f64),
                    _ => None,
                })
            })
            .unwrap_or(120.0);
        let notes: Vec<Note> = a
            .notes
            .iter()
            .map(|n| Note {
                note: n.note,
                start_beats: n.start_beats,
                length_beats: n.length_beats,
                velocity: n.velocity.unwrap_or(100),
            })
            .collect();
        let events = midi::plan(&notes, tempo).map_err(|e| bad(e.0))?;
        let clip_secs = a.length_beats * 60.0 / tempo;
        if events.iter().any(|e| e.at_secs > clip_secs + 1e-6) {
            return Err(bad("a note ends after length_beats"));
        }

        self.osc(&format!("/track/{t}/select"), vec![OscType::Int(1)])
            .await?;
        self.osc(&format!("/track/{t}/recarm"), vec![OscType::Int(1)])
            .await?;
        #[allow(clippy::cast_possible_truncation)]
        let len = a.length_beats as f32;
        self.osc(
            &format!("/track/{t}/clip/{s}/create"),
            vec![OscType::Float(len)],
        )
        .await?;
        let start = tokio::time::Instant::now();
        for e in &events {
            tokio::time::sleep_until(start + Duration::from_secs_f64(e.at_secs)).await;
            self.send_note(ch, e.note, e.velocity).await?;
        }
        Ok(format!(
            "recorded {} notes into clip {t}/{s} ({} beats at {tempo} bpm)",
            notes.len(),
            a.length_beats
        ))
    }

    // ------------------------------------------------------------ escape hatch

    #[tool(
        description = "Send any OSC message to Bitwig (see the DrivenByMoss OSC reference). Args are JSON numbers, strings or booleans; use 0/1 for flags. Values 0..MAX_VALUE (default 0-127)."
    )]
    async fn osc_send(&self, Parameters(a): Parameters<OscSendArgs>) -> ToolResult {
        let args: Vec<OscType> = a
            .args
            .iter()
            .map(|v| match v {
                Json::Bool(b) => Ok(flag(*b)),
                Json::Number(n) => Ok(parse_arg(&n.to_string())),
                Json::String(s) => Ok(OscType::String(s.clone())),
                other => Err(bad(format!("unsupported OSC argument: {other}"))),
            })
            .collect::<Result<_, _>>()?;
        self.run(&a.address, args, format!("sent {}", a.address))
            .await
    }
}

#[tool_handler(router = self.tool_router, name = "bitwig-mcp-rs-osc", instructions = "Controls Bitwig Studio through the DrivenByMoss Open Sound Control extension. \
Call get_overview first to see tracks, scenes, the transport and the selected device. \
Track/scene/slot/parameter indices are 1-based positions in the current bank window \
(default 8 items); move the window with scroll_track_bank / scroll_scene_bank. \
Levels are normalized 0.0-1.0 (pan 0.5 = center; volume ~0.79 is about 0 dB). \
Parameters of a device are reached by selecting the device (select_device_*) then \
set_device_param. Notes cannot be written into a clip directly: use record_midi_clip, which \
records them live, or play_notes to audition. osc_send / osc_get reach anything else.")]
impl rmcp::ServerHandler for OscServer {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::osc::wire::decode;
    use rosc::OscMessage;
    use tokio::net::UdpSocket;

    struct Rig {
        fake: UdpSocket,
        server: OscServer,
    }

    impl Rig {
        async fn new() -> Self {
            let fake = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
            let cfg = Config {
                send_port: fake.local_addr().expect("addr").port(),
                listen_port: 0,
                ..Config::default()
            };
            let bw = Arc::new(Bitwig::connect(&cfg).await.expect("connect"));
            Self {
                fake,
                server: OscServer::new(bw),
            }
        }

        async fn next(&self) -> OscMessage {
            let mut buf = [0u8; 1024];
            let (n, _) =
                tokio::time::timeout(Duration::from_secs(2), self.fake.recv_from(&mut buf))
                    .await
                    .expect("no datagram")
                    .expect("recv");
            decode(&buf[..n]).remove(0)
        }

        async fn nothing_sent(&self) -> bool {
            let mut buf = [0u8; 1024];
            tokio::time::timeout(Duration::from_millis(100), self.fake.recv_from(&mut buf))
                .await
                .is_err()
        }
    }

    #[tokio::test]
    async fn volume_is_scaled_to_the_wire_range() {
        let r = Rig::new().await;
        r.server
            .set_track_volume(Parameters(TrackLevelArgs {
                track: 3,
                value: 0.5,
            }))
            .await
            .expect("tool");
        let m = r.next().await;
        assert_eq!(m.addr, "/track/3/volume");
        assert_eq!(m.args, vec![OscType::Int(64)]);
    }

    #[tokio::test]
    async fn toggle_sends_no_argument_and_explicit_sends_a_flag() {
        let r = Rig::new().await;
        r.server
            .play(Parameters(FlagArgs { enabled: None }))
            .await
            .expect("tool");
        assert_eq!(r.next().await.args, vec![]);
        r.server
            .play(Parameters(FlagArgs {
                enabled: Some(false),
            }))
            .await
            .expect("tool");
        assert_eq!(r.next().await.args, vec![OscType::Int(0)]);
    }

    #[tokio::test]
    async fn bank_bounds_are_enforced_before_sending() {
        let r = Rig::new().await;
        let e = r
            .server
            .set_track_mute(Parameters(TrackFlagArgs {
                track: 9,
                enabled: None,
            }))
            .await
            .expect_err("9 is outside the default bank");
        assert!(e.message.contains("bank window"), "{e:?}");
        assert!(
            r.server
                .select_track(Parameters(TrackArgs { track: 0 }))
                .await
                .is_err()
        );
        assert!(r.nothing_sent().await);
    }

    #[tokio::test]
    async fn navigation_directions_map_to_plus_minus() {
        let r = Rig::new().await;
        r.server
            .scroll_track_bank(Parameters(ScrollArgs {
                direction: Direction::Next,
                page: Some(true),
            }))
            .await
            .expect("tool");
        assert_eq!(r.next().await.addr, "/track/bank/page/+");
        r.server
            .browser_filter(Parameters(BrowserFilterArgs {
                filter: 4,
                direction: Direction::Previous,
            }))
            .await
            .expect("tool");
        assert_eq!(r.next().await.addr, "/browser/filter/4/-");
        assert!(
            r.server
                .browser_filter(Parameters(BrowserFilterArgs {
                    filter: 7,
                    direction: Direction::Next
                }))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn tempo_range_is_validated() {
        let r = Rig::new().await;
        assert!(
            r.server
                .set_tempo(Parameters(TempoArgs { bpm: 5.0 }))
                .await
                .is_err()
        );
        r.server
            .set_tempo(Parameters(TempoArgs { bpm: 128.5 }))
            .await
            .expect("tool");
        let m = r.next().await;
        assert_eq!(
            (m.addr.as_str(), m.args),
            ("/tempo/raw", vec![OscType::Float(128.5)])
        );
    }

    #[tokio::test]
    async fn play_notes_sends_on_then_off() {
        let r = Rig::new().await;
        r.server
            .play_notes(Parameters(PlayNotesArgs {
                notes: vec![60, 64],
                velocity: Some(90),
                duration_ms: Some(10),
                channel: Some(2),
            }))
            .await
            .expect("tool");
        let mut got = Vec::new();
        for _ in 0..4 {
            let m = r.next().await;
            got.push((m.addr, m.args));
        }
        assert_eq!(
            got[0],
            ("/vkb_midi/2/note/60".into(), vec![OscType::Int(90)])
        );
        assert_eq!(
            got[1],
            ("/vkb_midi/2/note/64".into(), vec![OscType::Int(90)])
        );
        assert_eq!(got[2].1, vec![OscType::Int(0)]);
        assert_eq!(got[3].1, vec![OscType::Int(0)]);
    }

    #[tokio::test]
    async fn record_midi_clip_validates_then_creates_and_plays() {
        let r = Rig::new().await;
        let notes = |start: f64, len: f64| {
            vec![NoteSpec {
                note: 60,
                start_beats: start,
                length_beats: len,
                velocity: None,
            }]
        };
        let args = |notes| RecordClipArgs {
            track: 1,
            slot: 2,
            length_beats: 1.0,
            notes,
            channel: None,
            tempo_bpm: Some(600.0),
        };
        // ends after the clip: rejected, nothing sent
        assert!(
            r.server
                .record_midi_clip(Parameters(args(notes(0.5, 2.0))))
                .await
                .is_err()
        );
        assert!(r.nothing_sent().await);

        r.server
            .record_midi_clip(Parameters(args(notes(0.0, 0.5))))
            .await
            .expect("tool");
        let addrs: Vec<String> = {
            let mut v = Vec::new();
            for _ in 0..5 {
                v.push(r.next().await.addr);
            }
            v
        };
        assert_eq!(
            addrs,
            [
                "/track/1/select",
                "/track/1/recarm",
                "/track/1/clip/2/create",
                "/vkb_midi/1/note/60",
                "/vkb_midi/1/note/60"
            ]
        );
    }

    #[tokio::test]
    async fn osc_send_converts_json_arguments() {
        let r = Rig::new().await;
        r.server
            .osc_send(Parameters(OscSendArgs {
                address: "/x".into(),
                args: vec![
                    Json::from(3),
                    Json::from(1.5),
                    Json::from("hi"),
                    Json::Bool(true),
                ],
            }))
            .await
            .expect("tool");
        assert_eq!(
            r.next().await.args,
            vec![
                OscType::Int(3),
                OscType::Float(1.5),
                OscType::String("hi".into()),
                OscType::Int(1)
            ]
        );
        assert!(
            r.server
                .osc_send(Parameters(OscSendArgs {
                    address: "nope".into(),
                    args: vec![]
                }))
                .await
                .is_err()
        );
        assert!(
            r.server
                .osc_send(Parameters(OscSendArgs {
                    address: "/x".into(),
                    args: vec![Json::Null]
                }))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reading_without_bitwig_reports_a_helpful_error() {
        let r = Rig::new().await;
        // The fake never answers /refresh; shorten nothing: ensure_state waits 2.5 s.
        let e = r.server.get_overview().await.expect_err("no state");
        assert!(e.message.contains("DrivenByMoss"), "{e:?}");
        // and it asked for a refresh
        assert_eq!(r.next().await.addr, "/refresh");
    }

    #[tokio::test]
    async fn reading_returns_the_cached_state() {
        let r = Rig::new().await;
        let to = r.server.bw.listen_addr().expect("listening").port();
        for (addr, arg) in [
            ("/track/1/exists", OscType::Int(1)),
            ("/track/1/name", OscType::String("Kick".into())),
        ] {
            let bytes = crate::osc::wire::encode(addr, vec![arg]).expect("encode");
            r.fake
                .send_to(&bytes, ("127.0.0.1", to))
                .await
                .expect("send");
        }
        assert!(
            r.server
                .bw
                .wait_for(Duration::from_secs(2), |s| s.get("/track/1/name").is_some())
                .await
        );
        let out = r.server.get_mixer().await.expect("tool");
        assert!(out.contains("Kick"), "{out}");
    }

    #[test]
    fn every_tool_is_registered_with_a_description() {
        let tools = OscServer::tool_router().list_all();
        assert!(tools.len() > 90, "only {} tools", tools.len());
        for t in &tools {
            assert!(
                t.description.as_deref().is_some_and(|d| !d.is_empty()),
                "{}",
                t.name
            );
        }
        let mut names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "duplicate tool names");
    }
}
