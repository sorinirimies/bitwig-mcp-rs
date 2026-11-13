//! The MCP server on the bridge backend: tools, resources and prompts: `tools/` one module per area, `prompts.rs`, `helpers.rs`.
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
use crate::bridge::protocol::{
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

pub mod helpers;
pub mod prompts;
#[cfg(test)]
mod testkit;
pub mod tools;

use self::helpers::*;
use self::tools::clips::*;
use self::tools::devices::*;
use self::tools::studio::*;

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
    /// Every tool of every module in one router.
    fn all_tools() -> ToolRouter<Self> {
        Self::studio_router()
            + Self::transport_router()
            + Self::tracks_router()
            + Self::clips_router()
            + Self::devices_router()
            + Self::compose_router()
            + Self::hardware_router()
            + Self::export_router()
    }

    /// Wraps a bridge client; instrument profiles are read from `profiles_dir` on use.
    #[must_use]
    pub fn new(bridge: Arc<Bridge>, profiles_dir: PathBuf) -> Self {
        Self {
            bridge,
            profiles_dir,
            tool_router: Self::all_tools(),
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
                serde_json::to_string_pretty(&crate::bridge::protocol::schema()).map_err(err)?
            }
            _ => return Err(McpError::resource_not_found(request.uri, None)),
        };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::*;
    use crate::bridge::server::tools::{tracks::SetTrackArgs, transport::SetTransportArgs};

    #[test]
    fn every_tool_has_a_description_and_the_set_is_small() {
        let tools = BridgeServer::all_tools().list_all();
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
}
