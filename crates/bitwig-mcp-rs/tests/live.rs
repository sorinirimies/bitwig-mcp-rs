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
        let mut child = Command::new(env!("CARGO_BIN_EXE_bitwig-mcp-rs"))
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
        let _ = self.child.kill();
    }
}

const NAME: &str = "mcp-live-test";

#[test]
#[ignore = "needs Bitwig with the bridge extension; run with `just test-live`"]
fn build_play_and_clean_up() {
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
        json!({"track": NAME, "slot": slot, "resolution": "1/4"}),
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
