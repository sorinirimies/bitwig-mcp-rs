//! End-to-end: the real binary, real stdio.

use std::io::Write;
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
fn unknown_argument_is_a_usage_error() {
    let out = bin().arg("--nope").output().expect("run");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn reads_stdin() {
    let mut child = bin()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"# c\na = 1\n\nb=2\n")
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a\t1\nb\t2\n");
}
