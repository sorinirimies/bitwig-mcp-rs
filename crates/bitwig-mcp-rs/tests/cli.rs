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

/// Starts the server with `args`, does the MCP handshake and returns the tool names and the
/// server name.
fn tools(args: &[&str]) -> (String, Vec<String>) {
    let mut child = bin()
        .args(args)
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
    let name = init["result"]["serverInfo"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
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
    let names = tools["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    drop(stdin);
    let _ = child.wait();
    (name, names)
}

#[test]
fn bridge_backend_speaks_mcp_without_bitwig() {
    // Nothing listens on the bridge port: the server must still start and list its tools.
    let (name, names) = tools(&["--bridge-port", "1"]);
    assert_eq!(name, "bitwig-mcp-rs");
    for want in [
        "get_project",
        "create_track",
        "set_track",
        "create_clip",
        "hardware_set",
    ] {
        assert!(names.iter().any(|n| n == want), "missing {want}");
    }
}

#[test]
fn osc_backend_speaks_mcp() {
    let (name, names) = tools(&[
        "--backend",
        "osc",
        "--send-port",
        "18000",
        "--listen-port",
        "0",
    ]);
    assert_eq!(name, "bitwig-mcp-rs-osc");
    for want in [
        "get_overview",
        "play",
        "set_track_volume",
        "record_midi_clip",
        "osc_send",
    ] {
        assert!(names.iter().any(|n| n == want), "missing {want}");
    }
}

#[test]
fn install_extension_writes_the_bundled_extension() {
    let dir = std::env::temp_dir().join(format!("bwmcp-cli-ext-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let out = bin()
        .args(["install-extension", "--dir"])
        .arg(&dir)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("installed"), "{text}");
    assert!(
        text.contains("MCP bridge"),
        "explains the controller step: {text}"
    );
    let jar = std::fs::read(dir.join("bitwig-mcp-rs.bwextension")).expect("written");
    assert_eq!(&jar[..2], b"PK");
    let again = bin()
        .args(["install-extension", "--dir"])
        .arg(&dir)
        .output()
        .expect("run");
    assert!(String::from_utf8_lossy(&again.stdout).contains("current"));
    std::fs::remove_dir_all(&dir).ok();
}
