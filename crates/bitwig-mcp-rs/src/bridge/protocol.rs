//! The bridge wire protocol: what `bitwig-mcp-rs` and the Bitwig extension say to each other.
//!
//! Transport: one TCP connection to the extension (default port 8417). Every message is a
//! frame: a 32-bit big-endian length, then that many bytes of UTF-8 JSON (Bitwig's
//! `RemoteConnection` framing).
//!
//! * Requests: `{"id": 7, "cmd": "set_track", ...fields}` ([`Request`]).
//! * Replies: `{"id": 7, "ok": true, "result": ...}` or `{"id": 7, "ok": false, "error": "..."}`
//!   ([`Reply`]).
//!
//! All indices on the wire are **0-based**; the MCP tools show 1-based numbers. Levels are
//! normalized `0.0..=1.0`. The extension stays dumb: name lookup, dB conversion and note-grid
//! math happen in Rust. `bitwig-bridge/protocol.schema.json` is generated from these types
//! (a test keeps it current) and documents the protocol for the Java side.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Protocol version; bumped on incompatible changes. The extension reports its own in `hello`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame accepted (16 MiB): a project snapshot is far below that.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Which track list a track lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackBank {
    /// Audio, instrument and group tracks, flattened (group children included).
    Main,
    /// Effect (return) tracks.
    Effect,
    /// The master track (index is always 0).
    Master,
}

/// A resolved track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackAddr {
    /// The list.
    pub bank: TrackBank,
    /// 0-based position in that list.
    pub index: u32,
}

/// RGB color, 0-255 per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Kind of track to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Audio track.
    Audio,
    /// Instrument track.
    Instrument,
    /// Effect (return) track.
    Effect,
}

/// Transport actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    /// Start playback.
    Play,
    /// Stop playback.
    Stop,
    /// Toggle play/stop.
    TogglePlay,
    /// Toggle arranger recording.
    Record,
    /// Restart from the play start position.
    Restart,
    /// Rewind.
    Rewind,
    /// Fast forward.
    FastForward,
    /// Tap tempo once.
    TapTempo,
    /// Stop every clip in the launcher.
    StopAllClips,
    /// Add a cue marker at the play position.
    AddCueMarker,
    /// Jump to the next cue marker.
    NextCueMarker,
    /// Jump to the previous cue marker.
    PreviousCueMarker,
}

/// Actions on a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackAction {
    /// Select it (cursor track follows).
    Select,
    /// Duplicate it.
    Duplicate,
    /// Delete it.
    Delete,
    /// Stop its launcher clips.
    StopClips,
    /// Switch it back to arrangement playback.
    ReturnToArrangement,
    /// Scroll it into view in arranger and mixer.
    Show,
}

/// Actions on a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SceneAction {
    /// Launch every clip in the row.
    Launch,
    /// Select it.
    Select,
    /// Duplicate it.
    Duplicate,
    /// Delete it.
    Delete,
}

/// Actions on a clip slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipAction {
    /// Launch the clip.
    Launch,
    /// Stop the track's clips.
    Stop,
    /// Select the slot.
    Select,
    /// Start recording into the slot.
    Record,
    /// Delete the clip.
    Delete,
    /// Duplicate the clip.
    Duplicate,
    /// Open the clip in the editor.
    Show,
}

/// Actions on a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAction {
    /// Enable (un-bypass).
    Enable,
    /// Disable (bypass).
    Disable,
    /// Toggle enabled.
    Toggle,
    /// Delete it.
    Delete,
    /// Duplicate it.
    Duplicate,
    /// Open the plugin window.
    OpenWindow,
    /// Close the plugin window.
    CloseWindow,
    /// Load the next preset.
    NextPreset,
    /// Load the previous preset.
    PreviousPreset,
    /// Select it (cursor device follows).
    Select,
}

/// Where a new device goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "at", content = "device", rename_all = "snake_case")]
pub enum Insert {
    /// End of the track's chain.
    End,
    /// Start of the track's chain.
    Start,
    /// After the device at this 0-based index.
    After(u32),
    /// Before the device at this 0-based index.
    Before(u32),
    /// Replace the device at this 0-based index.
    Replace(u32),
}

/// What to insert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceSource {
    /// Search the browser for a result whose name matches (exact first, then substring),
    /// in the given browser tab ("Devices", "Presets", ...).
    Browser {
        /// Name to search for, case-insensitive.
        name: String,
        /// Browser content type; default "Devices".
        content_type: Option<String>,
    },
    /// A CLAP plugin by id.
    Clap {
        /// e.g. "org.surge-synth-team.surge-xt".
        id: String,
    },
    /// A VST3 plugin by id.
    Vst3 {
        /// The VST3 class id.
        id: String,
    },
    /// A file (preset, sample, ...) by absolute path.
    File {
        /// Absolute path.
        path: String,
    },
}

/// One note on the grid of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GridNote {
    /// Step index (start = x · step).
    pub x: u32,
    /// MIDI key 0-127.
    pub key: u8,
    /// Velocity 1-127.
    pub velocity: u8,
    /// Length in beats.
    pub duration: f64,
}

/// One send level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SendLevel {
    /// 0-based send index.
    pub index: u32,
    /// Normalized level.
    pub volume: f64,
}

/// A 3-byte MIDI message.
pub type Midi = [u8; 3];

/// Track fields to change; absent fields stay.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackChange {
    /// New name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// Volume, normalized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Pan, normalized (0.5 = center).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    /// Mute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// Solo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
    /// Record arm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arm: Option<bool>,
    /// Activated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activated: Option<bool>,
    /// Monitor mode: "ON", "OFF" or "AUTO".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_mode: Option<String>,
    /// Send levels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<SendLevel>,
}

/// Transport fields to change; absent fields stay.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransportChange {
    /// Tempo in BPM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    /// Arranger loop on/off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#loop: Option<bool>,
    /// Loop start in beats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_start: Option<f64>,
    /// Loop length in beats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_length: Option<f64>,
    /// Metronome on/off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metronome: Option<bool>,
    /// Play position in beats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    /// Time signature like "4/4".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<String>,
    /// Punch in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub punch_in: Option<bool>,
    /// Punch out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub punch_out: Option<bool>,
    /// Arranger overdub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arranger_overdub: Option<bool>,
    /// Launcher overdub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launcher_overdub: Option<bool>,
    /// Arranger automation write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_write: Option<bool>,
    /// "latch", "touch" or "write".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_mode: Option<String>,
    /// "none", "one_bar", "two_bars" or "four_bars".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_roll: Option<String>,
    /// Default launch quantization: "none", "8", "4", "2", "1", "1/2", "1/4", "1/8", "1/16".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_quantization: Option<String>,
}

/// Clip fields to change; absent fields stay.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipChange {
    /// New name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// Loop length in beats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_length: Option<f64>,
    /// Loop on/off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_enabled: Option<bool>,
    /// Transpose by semitones (applied once).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transpose: Option<i32>,
    /// Quantize amount 0-1 (applied once).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantize: Option<f64>,
}

/// Everything the Rust side can ask. Serialized with a `cmd` tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Handshake: returns `{protocol, extension_version, api_version}`.
    Hello,
    /// Full snapshot: transport, application, tracks, effect tracks, master, scenes.
    GetProject,
    /// Run a transport action.
    Transport {
        /// The action.
        action: TransportAction,
    },
    /// Change transport settings; replies with the transport snapshot.
    SetTransport {
        /// The changes.
        #[serde(flatten)]
        change: TransportChange,
    },
    /// Create a track; replies `{bank, index}` of the new track.
    CreateTrack {
        /// Kind.
        kind: TrackKind,
        /// 0-based position in the main list, -1 = end (ignored for effect tracks).
        position: i32,
    },
    /// Change a track; replies with its snapshot after the change settled.
    SetTrack {
        /// The track.
        track: TrackAddr,
        /// The changes.
        #[serde(flatten)]
        change: TrackChange,
    },
    /// Run an action on a track.
    TrackAction {
        /// The track.
        track: TrackAddr,
        /// The action.
        action: TrackAction,
    },
    /// Create a scene at the end; replies `{index}`.
    CreateScene,
    /// Change a scene.
    SetScene {
        /// 0-based scene.
        scene: u32,
        /// New name.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// New color.
        #[serde(skip_serializing_if = "Option::is_none")]
        color: Option<Rgb>,
    },
    /// Run an action on a scene.
    SceneAction {
        /// 0-based scene.
        scene: u32,
        /// The action.
        action: SceneAction,
    },
    /// Create an empty launcher clip.
    CreateClip {
        /// The track.
        track: TrackAddr,
        /// 0-based slot.
        slot: u32,
        /// Length in whole beats.
        length: u32,
    },
    /// Run an action on a clip slot.
    ClipAction {
        /// The track.
        track: TrackAddr,
        /// 0-based slot.
        slot: u32,
        /// The action.
        action: ClipAction,
    },
    /// Change a clip (focuses it in the editor).
    SetClip {
        /// The track.
        track: TrackAddr,
        /// 0-based slot.
        slot: u32,
        /// The changes.
        #[serde(flatten)]
        change: ClipChange,
    },
    /// Write notes into a launcher clip at exact grid positions.
    WriteNotes {
        /// The track.
        track: TrackAddr,
        /// 0-based slot (must hold a clip).
        slot: u32,
        /// Grid step in beats.
        step: f64,
        /// Remove existing notes first.
        clear: bool,
        /// The notes.
        notes: Vec<GridNote>,
    },
    /// Read the notes of a launcher clip on a grid.
    ReadNotes {
        /// The track.
        track: TrackAddr,
        /// 0-based slot.
        slot: u32,
        /// Grid step in beats.
        step: f64,
    },
    /// Insert a device; replies when the browser committed or the plugin was inserted.
    InsertDevice {
        /// The track.
        track: TrackAddr,
        /// Where.
        #[serde(flatten)]
        at: Insert,
        /// What.
        source: DeviceSource,
    },
    /// A device with its remote-control pages and the parameters of one page.
    GetDevice {
        /// The track.
        track: TrackAddr,
        /// 0-based device in the track's chain.
        device: u32,
        /// 0-based remote-control page; default: the current one.
        #[serde(skip_serializing_if = "Option::is_none")]
        page: Option<u32>,
    },
    /// Set a remote-control parameter; replies with its new value and display text.
    SetDeviceParam {
        /// The track.
        track: TrackAddr,
        /// 0-based device.
        device: u32,
        /// 0-based remote-control page; default: the current one.
        #[serde(skip_serializing_if = "Option::is_none")]
        page: Option<u32>,
        /// 0-based parameter on the page (0-7).
        param: u32,
        /// Normalized value.
        value: f64,
    },
    /// Run an action on a device.
    DeviceAction {
        /// The track.
        track: TrackAddr,
        /// 0-based device.
        device: u32,
        /// The action.
        action: DeviceAction,
    },
    /// Play notes on a track for a duration (audition, or capture while recording).
    PlayNotes {
        /// The track.
        track: TrackAddr,
        /// MIDI keys.
        keys: Vec<u8>,
        /// Velocity 1-127.
        velocity: u8,
        /// Hold time in milliseconds.
        duration_ms: u32,
    },
    /// Send raw MIDI messages to a track (e.g. on to a hardware instrument).
    SendMidi {
        /// The track.
        track: TrackAddr,
        /// The messages, in order.
        messages: Vec<Midi>,
    },
    /// Send a SysEx message through the extension's MIDI output port (Track MIDI cannot carry
    /// SysEx). Nothing is sent when no port is assigned to the controller.
    SendSysex {
        /// The complete message, `F0 … F7`.
        bytes: Vec<u8>,
    },
    /// The song plan stored in the project (a JSON text; empty when none).
    GetSongPlan,
    /// Store the song plan in the project (saved with it).
    SetSongPlan {
        /// JSON text, at most 60000 characters.
        text: String,
    },
    /// List Bitwig application actions (menu commands) whose id or name contains `query`.
    ListActions {
        /// Case-insensitive filter; empty = all.
        query: String,
    },
    /// Invoke an application action by id.
    InvokeAction {
        /// Action id from `list_actions`.
        id: String,
    },
    /// Undo the last edit.
    Undo,
    /// Redo.
    Redo,
    /// Change the window: layout and/or toggle a panel.
    SetView {
        /// "ARRANGE", "MIX" or "EDIT".
        #[serde(skip_serializing_if = "Option::is_none")]
        layout: Option<String>,
        /// "inspector", "devices", "mixer", "note_editor", "automation_editor", "browser",
        /// "fullscreen".
        #[serde(skip_serializing_if = "Option::is_none")]
        toggle: Option<String>,
    },
}

/// A request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    /// Correlates the reply.
    pub id: u64,
    /// The command.
    #[serde(flatten)]
    pub command: Command,
}

/// A reply frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Reply {
    /// The request id.
    pub id: u64,
    /// Success.
    pub ok: bool,
    /// Result on success.
    #[serde(default)]
    pub result: Json,
    /// Message on failure.
    #[serde(default)]
    pub error: Option<String>,
}

/// Framing failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The length header exceeds [`MAX_FRAME`].
    TooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "frame of {n} bytes exceeds the {MAX_FRAME} byte limit"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Prefixes a payload with its length.
///
/// # Panics
/// Never for payloads below 4 GiB; larger ones are a programming error.
#[must_use]
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Takes one complete frame off the front of `buf`, if there is one.
///
/// # Errors
/// [`FrameError::TooLarge`] for a length above [`MAX_FRAME`]; the stream is then unusable.
pub fn take_frame(buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>, FrameError> {
    let Some(header) = buf.get(..4) else {
        return Ok(None);
    };
    let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let payload = buf[4..4 + len].to_vec();
    buf.drain(..4 + len);
    Ok(Some(payload))
}

/// The JSON inside a frame payload. The extension writes its own length header and Bitwig's
/// `RemoteConnection` may add another; a payload that starts with a header matching its own
/// remaining length is unwrapped once.
#[must_use]
pub fn unwrap_payload(payload: &[u8]) -> &[u8] {
    match payload {
        [a, b, c, d, rest @ ..] if u32::from_be_bytes([*a, *b, *c, *d]) as usize == rest.len() => {
            rest
        }
        _ => payload,
    }
}

/// JSON Schema of the protocol (requests and replies).
#[must_use]
pub fn schema() -> Json {
    let schema = schemars::schema_for!(Request);
    let reply = schemars::schema_for!(Reply);
    serde_json::json!({
        "title": "bitwig-mcp-rs bridge protocol",
        "protocol_version": PROTOCOL_VERSION,
        "framing": "u32 big-endian length + UTF-8 JSON",
        "request": schema,
        "reply": reply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_a_flat_cmd_tag() {
        let r = Request {
            id: 3,
            command: Command::SetTrack {
                track: TrackAddr {
                    bank: TrackBank::Main,
                    index: 0,
                },
                change: TrackChange {
                    volume: Some(0.5),
                    ..TrackChange::default()
                },
            },
        };
        let v = serde_json::to_value(&r).expect("json");
        assert_eq!(
            v,
            serde_json::json!({
                "id": 3, "cmd": "set_track",
                "track": {"bank": "main", "index": 0},
                "volume": 0.5
            })
        );
        let back: Request = serde_json::from_value(v).expect("parse");
        assert_eq!(back, r);
    }

    #[test]
    fn insert_and_source_shapes() {
        let c = Command::InsertDevice {
            track: TrackAddr {
                bank: TrackBank::Main,
                index: 1,
            },
            at: Insert::After(2),
            source: DeviceSource::Browser {
                name: "Polymer".into(),
                content_type: None,
            },
        };
        let v = serde_json::to_value(&c).expect("json");
        assert_eq!(v["at"], "after");
        assert_eq!(v["device"], 2);
        assert_eq!(v["source"]["kind"], "browser");
        let end = serde_json::to_value(Command::InsertDevice {
            track: TrackAddr {
                bank: TrackBank::Main,
                index: 1,
            },
            at: Insert::End,
            source: DeviceSource::Clap { id: "x".into() },
        })
        .expect("json");
        assert_eq!(end["at"], "end");
        assert!(end.get("device").is_none());
    }

    #[test]
    fn unit_commands_are_just_the_tag() {
        let v = serde_json::to_value(Request {
            id: 1,
            command: Command::Hello,
        })
        .expect("json");
        assert_eq!(v, serde_json::json!({"id": 1, "cmd": "hello"}));
    }

    #[test]
    fn replies_parse_both_ways() {
        let ok: Reply = serde_json::from_str(r#"{"id":1,"ok":true,"result":{"a":1}}"#).expect("ok");
        assert!(ok.ok);
        assert_eq!(ok.result["a"], 1);
        let err: Reply =
            serde_json::from_str(r#"{"id":2,"ok":false,"error":"nope"}"#).expect("err");
        assert_eq!(err.error.as_deref(), Some("nope"));
    }

    #[test]
    fn framing_round_trip_and_partial_reads() {
        let mut buf = frame(b"hello");
        buf.extend(frame(b""));
        buf.extend(&frame(b"xyz")[..5]); // incomplete third frame
        assert_eq!(take_frame(&mut buf), Ok(Some(b"hello".to_vec())));
        assert_eq!(take_frame(&mut buf), Ok(Some(Vec::new())));
        assert_eq!(take_frame(&mut buf), Ok(None));
        buf.extend(b"yz");
        assert_eq!(take_frame(&mut buf), Ok(Some(b"xyz".to_vec())));
        assert!(buf.is_empty());
    }

    #[test]
    fn double_headers_are_unwrapped_once() {
        let inner = frame(br#"{"id":1}"#);
        assert_eq!(unwrap_payload(&inner), br#"{"id":1}"#);
        assert_eq!(unwrap_payload(br#"{"id":1}"#), br#"{"id":1}"#);
        assert_eq!(unwrap_payload(b"ab"), b"ab");
        assert_eq!(unwrap_payload(&[0, 0, 0, 0]), b"");
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut buf = u32::MAX.to_be_bytes().to_vec();
        assert!(matches!(take_frame(&mut buf), Err(FrameError::TooLarge(_))));
    }

    #[test]
    fn checked_in_schema_is_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../bitwig-bridge/protocol.schema.json"
        );
        let want = serde_json::to_string_pretty(&schema()).expect("json") + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(path, &want).expect("write schema");
        }
        let have = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            have == want,
            "bitwig-bridge/protocol.schema.json is stale: run `just schema`"
        );
    }
}
