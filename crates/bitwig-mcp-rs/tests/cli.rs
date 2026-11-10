//! End-to-end: the real binary, real stdio, speaking MCP.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bitwig-mcp-rs"))
}

#[test]
fn version_matches_the_crate() {
    let out = bin().arg("--version").output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("bitwig-mcp-rs {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_lists_the_options() {
    let out = bin().arg("--help").output().expect("run");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--send-port"));
}

#[test]
fn unknown_argument_is_a_usage_error() {
    let out = bin().arg("--nope").output().expect("run");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn bad_port_is_a_usage_error() {
    let out = bin().args(["--send-port", "banana"]).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
}

/// Sends one JSON-RPC line and reads one line back.
fn rpc(
    stdin: &mut impl Write,
    stdout: &mut impl BufRead,
    request: &serde_json::Value,
) -> serde_json::Value {
    writeln!(stdin, "{request}").expect("write");
    stdin.flush().expect("flush");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read");
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"))
}

#[test]
fn speaks_mcp_and_lists_tools() {
    let mut child = bin()
        // ports that nothing uses; the server must still start and answer MCP
        .args(["--send-port", "18000", "--listen-port", "0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let init = rpc(
        &mut stdin,
        &mut stdout,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }
        }),
    );
    assert_eq!(
        init["result"]["serverInfo"]["name"], "bitwig-mcp-rs",
        "{init}"
    );

    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .expect("write");

    let tools = rpc(
        &mut stdin,
        &mut stdout,
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for want in [
        "get_overview",
        "play",
        "set_track_volume",
        "record_midi_clip",
        "osc_send",
    ] {
        assert!(names.contains(&want), "missing {want}");
    }

    drop(stdin);
    let _ = child.wait();
}
