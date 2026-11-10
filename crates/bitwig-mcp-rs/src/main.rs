use std::io::{self, BufRead, Write};
use std::process::ExitCode;

const HELP: &str = "\
bitwig-mcp-rs — Rust MCP server that controls Bitwig Studio over OSC (DrivenByMoss)

Usage: bitwig-mcp-rs [OPTIONS]
  Reads `key = value` lines on stdin and prints `key<TAB>value`.

Options:
  -h, --help     Print help
  -V, --version  Print version";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("-V" | "--version") => {
            println!("bitwig-mcp-rs {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("-h" | "--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n\n{HELP}");
            ExitCode::from(2)
        }
        None => match run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run() -> io::Result<()> {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        if let Some((k, v)) = bitwig_mcp_rs::parse_kv(&line?) {
            writeln!(out, "{k}\t{v}")?;
        }
    }
    Ok(())
}
