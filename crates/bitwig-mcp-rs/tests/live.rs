//! Live end-to-end test against a running Bitwig with the bridge extension added.
//! Ignored by default: `just test-live` (or `cargo test --test live -- --ignored --nocapture`).
//!
//! It creates a track and a scene named `mcp-live-test`, works with them, and deletes both.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Session {
    fn start() -> Self {
        // BITWIG_MCP_BIN tests an installed binary (e.g. `cargo install bitwig-mcp-rs`)
        // instead of this build: what a real user runs.
        let bin = std::env::var("BITWIG_MCP_BIN")
            .unwrap_or_else(|_| env!("CARGO_BIN_EXE_bitwig-mcp-rs").to_owned());
        eprintln!("testing {bin}");
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut s = Self {
            child,
            stdin,
            stdout,
            next: 1,
        };
        s.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "live", "version": "0"}}),
        );
        writeln!(
            s.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .expect("write");
        s
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .expect("write");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"))
    }

    /// Calls a tool; returns its JSON text parsed, panicking on tool errors.
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let result = &r["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            r.get("error").is_none() && result["isError"] != true,
            "{name} failed: {r}"
        );
        eprintln!("{name} → {}", text.chars().take(300).collect::<String>());
        serde_json::from_str(text).unwrap_or(Value::String(text.to_owned()))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Runs on failure too: remove what the test created, ignoring errors.
        if std::thread::panicking() {
            for (name, args) in [
                ("scene_action", json!({"scene": NAME, "action": "delete"})),
                ("track_action", json!({"track": NAME, "action": "delete"})),
                ("scene_action", json!({"scene": F_S1, "action": "delete"})),
                ("scene_action", json!({"scene": F_S2, "action": "delete"})),
                (
                    "track_action",
                    json!({"track": F_TRACK, "action": "delete"}),
                ),
                ("track_action", json!({"track": F_FX, "action": "delete"})),
                (
                    "track_action",
                    json!({"track": F_DRUMS, "action": "delete"}),
                ),
            ] {
                let id = self.next;
                self.next += 1;
                let msg = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                                 "params": {"name": name, "arguments": args}});
                if writeln!(self.stdin, "{msg}").is_ok() {
                    let mut line = String::new();
                    let _ = self.stdout.read_line(&mut line);
                }
            }
        }
        let _ = self.child.kill();
    }
}

const NAME: &str = "mcp-live-test";
const F_TRACK: &str = "mcp-live-melody";
const F_FX: &str = "mcp-live-fx";
const F_DRUMS: &str = "mcp-live-drums";
const F_S1: &str = "mcp-live-s1";
const F_S2: &str = "mcp-live-s2";

/// Both live tests share one Bitwig (one selection, one browser): never in parallel.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
#[ignore = "needs Bitwig with the bridge extension; run with `just test-live`"]
fn build_play_and_clean_up() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::start();

    let status = s.tool("status", json!({}));
    assert_eq!(status["connected"], true, "{status}");

    // a track with a device, named and colored
    let created = s.tool(
        "create_track",
        json!({"kind": "instrument", "name": NAME, "color": "teal", "device": "Polymer"}),
    );
    assert_eq!(created["track"]["name"], NAME, "{created}");
    assert_eq!(created["warnings"], json!([]), "{created}");

    // mixer with dB, confirmed by Bitwig's own display text
    let t = s.tool(
        "set_track",
        json!({"track": NAME, "volume_db": -6.0, "pan": 0.25}),
    );
    let text = t["volume_text"].as_str().unwrap_or_default();
    assert!(
        text.contains("-6.0") || text.contains("-5.9") || text.contains("-6.1"),
        "{t}"
    );

    // a scene and a clip with exact notes
    let scene = s.tool("create_scene", json!({"name": NAME}));
    let slot = scene["scene"].clone();
    s.tool(
        "create_clip",
        json!({"track": NAME, "slot": slot, "length_beats": 4, "name": "riff",
        "notes": [
            {"note": "C3", "start": 0, "length": 0.5},
            {"note": "E3", "start": 1, "length": 0.5, "velocity": 90},
            {"note": "G3", "start": 2.5, "length": 1}
        ]}),
    );
    let read = s.tool(
        "read_notes",
        json!({"track": NAME, "slot": slot, "resolution": "1/8"}),
    );
    let starts: Vec<f64> = read["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .filter_map(|n| n["start"].as_f64())
        .collect();
    assert_eq!(starts, vec![0.0, 1.0, 2.5], "{read}");

    // device parameters by name
    let dev = s.tool("get_device", json!({"track": NAME, "device": "polymer"}));
    let first = dev["params"][0]["name"]
        .as_str()
        .expect("a parameter")
        .to_owned();
    let p = s.tool(
        "set_device_param",
        json!({"track": NAME, "device": 1, "param": first, "value": 0.5}),
    );
    assert!(
        (p["value"].as_f64().unwrap_or(-1.0) - 0.5).abs() < 0.02,
        "{p}"
    );

    s.tool(
        "play_notes",
        json!({"track": NAME, "notes": ["C3", "E3", "G3"], "duration_ms": 200}),
    );
    assert!(
        s.tool("list_actions", json!({"query": "save"}))
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );

    // clean up
    s.tool("scene_action", json!({"scene": NAME, "action": "delete"}));
    s.tool("track_action", json!({"track": NAME, "action": "delete"}));
    let project = s.tool("get_project", json!({}));
    assert!(!project.to_string().contains(NAME), "left-overs: {project}");
}

/// Notes of a clip as sorted MIDI keys.
fn keys(s: &mut Session, track: &str, slot: &Value) -> Vec<u64> {
    let read = s.tool("read_notes", json!({"track": track, "slot": slot}));
    let mut k: Vec<u64> = read["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .filter_map(|n| n["key"].as_u64().or_else(|| n["note"].as_u64()))
        .collect();
    k.sort_unstable();
    k
}

#[test]
#[ignore = "needs Bitwig with the bridge extension; run with `just test-live`"]
fn composing_chains_meters_and_recording() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::start();
    let status = s.tool("status", json!({}));
    assert_eq!(status["connected"], true, "{status}");
    assert!(status["server_version"].is_string(), "{status}");
    let new_extension = status["extension"]["extension_version"]
        .as_str()
        .is_some_and(|v| v >= "0.2");
    if !new_extension {
        eprintln!("old extension: skipping meters, pad effects and recording; restart Bitwig");
    }

    s.tool(
        "create_track",
        json!({"kind": "instrument", "name": F_TRACK, "device": "Polymer"}),
    );
    let s1 = s.tool("create_scene", json!({"name": F_S1}))["scene"].clone();
    let s2 = s.tool("create_scene", json!({"name": F_S2}))["scene"].clone();

    // a hook in the key, over chords
    s.tool(
        "write_melody",
        json!({"track": F_TRACK, "slot": s1, "key": "A minor", "progression": "i VI III VII",
               "seed": 3, "name": "hook"}),
    );
    let hook = keys(&mut s, F_TRACK, &s1);
    assert!(hook.len() >= 8, "{hook:?}");
    let a_minor = [9, 11, 0, 2, 4, 5, 7];
    assert!(hook.iter().all(|k| a_minor.contains(&(k % 12))), "{hook:?}");

    // a varied copy in another scene
    s.tool(
        "copy_clip",
        json!({"track": F_TRACK, "slot": s1,
               "to": [{"slot": s2, "ops": [{"op": "transpose", "semitones": 12}]}]}),
    );
    let copy = keys(&mut s, F_TRACK, &s2);
    let up: Vec<u64> = hook.iter().map(|k| k + 12).collect();
    assert_eq!(copy, up);

    // several parameters at once
    let dev = s.tool("get_device", json!({"track": F_TRACK, "device": 1}));
    let names: Vec<String> = dev["params"]
        .as_array()
        .expect("params")
        .iter()
        .take(2)
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect();
    let set = s.tool(
        "set_device_params",
        json!({"track": F_TRACK, "device": 1,
               "params": names.iter().map(|n| json!({"param": n, "value": 0.4})).collect::<Vec<_>>()}),
    );
    assert_eq!(set.as_array().map(Vec::len), Some(names.len()), "{set}");

    // an effect chain, and a reverb bus with a send
    let chain = s.tool(
        "insert_chain",
        json!({"track": F_TRACK, "devices": [{"name": "EQ-5"}, {"name": "Compressor"}]}),
    );
    assert!(!chain.to_string().contains("error"), "{chain}");
    let fx = s.tool(
        "create_return",
        json!({"name": F_FX, "devices": [{"name": "Reverb"}],
               "sends": [{"track": F_TRACK, "volume_db": -12}]}),
    );
    assert!(!fx["sends"].to_string().contains("error"), "{fx}");

    // the summary is small, the detail is not
    let summary = s.tool("get_project", json!({}));
    assert_eq!(summary["summary"], true);
    let detail = s.tool("get_project", json!({"detail": true}));
    assert!(detail.to_string().len() > summary.to_string().len());

    // build the song from parts
    let built = s.tool(
        "build_song",
        json!({"sections": [{"name": F_S2, "parts": [
            {"kind": "chords", "track": F_TRACK, "progression": "i VI", "key": "A minor"}]}]}),
    );
    assert_eq!(built["failed"], 0, "{built}");

    if new_extension {
        // drum pad effects
        s.tool(
            "create_track",
            json!({"kind": "instrument", "name": F_DRUMS, "device": "Drum Machine"}),
        );
        let kit = s.tool(
            "build_drum_kit",
            json!({"track": F_DRUMS, "pads": {"snare": "v8 Snare"},
                   "effects": {"snare": ["Reverb"]}}),
        );
        assert!(!kit["effects"].to_string().contains("error"), "{kit}");

        // meters, an arrangement take and an automation take (short: one bar each)
        s.tool("scene_action", json!({"scene": F_S1, "action": "launch"}));
        let m = s.tool("get_meters", json!({"window_ms": 600}));
        assert!(m["meters"].as_array().is_some_and(|a| !a.is_empty()), "{m}");
        s.tool("transport", json!({"action": "stop"}));
        let rec = s.tool(
            "record_arrangement",
            json!({"scenes": [{"scene": F_S1, "bars": 1}]}),
        );
        assert_eq!(rec["recorded_bars"], 1, "{rec}");
        let auto = s.tool(
            "record_automation",
            json!({"track": F_TRACK, "device": 1, "param": names[0], "from": 0.2, "to": 0.8,
                   "bars": 1, "curve": "ease_in"}),
        );
        assert!(auto["steps"].as_u64().unwrap_or(0) >= 4, "{auto}");
    }

    // clean up
    s.tool("scene_action", json!({"scene": F_S1, "action": "delete"}));
    s.tool("scene_action", json!({"scene": F_S2, "action": "delete"}));
    for t in [F_TRACK, F_FX, F_DRUMS] {
        if new_extension || t != F_DRUMS {
            s.tool("track_action", json!({"track": t, "action": "delete"}));
        }
    }
    let project = s.tool("get_project", json!({}));
    assert!(
        !project.to_string().contains("mcp-live"),
        "left-overs: {project}"
    );
}
