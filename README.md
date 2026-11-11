# Bitwig MCP

[![CI](https://github.com/sorinirimies/bitwig-mcp-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/sorinirimies/bitwig-mcp-rs/actions/workflows/ci.yml)
[![Security](https://github.com/sorinirimies/bitwig-mcp-rs/actions/workflows/security.yml/badge.svg)](https://github.com/sorinirimies/bitwig-mcp-rs/actions/workflows/security.yml)
[![Fuzz](https://github.com/sorinirimies/bitwig-mcp-rs/actions/workflows/fuzz.yml/badge.svg)](https://github.com/sorinirimies/bitwig-mcp-rs/actions/workflows/fuzz.yml)
[![crates.io](https://img.shields.io/crates/v/bitwig-mcp-rs.svg)](https://crates.io/crates/bitwig-mcp-rs)

Rust MCP server that controls Bitwig Studio over OSC (DrivenByMoss)

## Install

```sh
cargo install bitwig-mcp-rs
```

or download the archive for your platform (macOS arm64/x86-64, Linux arm64/x86-64
static, Windows x86-64) from the [latest release](https://github.com/sorinirimies/bitwig-mcp-rs/releases).

## How it works

```
MCP client (Claude, Zed, pi, ...) ⇄ stdio/MCP ⇄ bitwig-mcp-rs ⇄ TCP/JSON ⇄ MCP bridge extension (in Bitwig)
```

Two backends:

| | **bridge** (default) | **osc** (`--backend osc`) |
|---|---|---|
| Bitwig side | our extension, `bitwig-bridge/` | [DrivenByMoss](https://www.mossgrabers.de/Software/Bitwig/Bitwig.html) "Open Sound Control" |
| Tracks | all (64 main + 16 effect + master), by name | a window of 8, by bank position |
| Clip notes | written and read exactly | recorded live only |
| Devices | load by name/preset/CLAP/VST3, every remote-control page | browser stepping |
| Tools | ~30, names or numbers, dB, replies with Bitwig's state | ~100 low-level |

### 1. Install the bridge extension

```sh
just bridge-install     # needs a JDK (17+); builds target/bridge/bitwig-mcp-rs.bwextension
```

copies it into `~/Bitwig Studio/Extensions` (Linux) or `~/Documents/Bitwig Studio/Extensions`
(macOS, Windows). Then in Bitwig: *Settings → Controllers → Add controller →
bitwig-mcp-rs → MCP bridge*. It listens on TCP port 8417 (changeable in the controller's
settings).

### 2. Add the server to your MCP client

```json
{
  "mcpServers": {
    "bitwig": { "command": "bitwig-mcp-rs" }
  }
}
```

Options (flag or environment variable): `--backend` / `BITWIG_BACKEND` (bridge),
`--host` / `BITWIG_HOST` (127.0.0.1), `--bridge-port` / `BITWIG_BRIDGE_PORT` (8417),
`--instruments-dir` / `BITWIG_INSTRUMENTS_DIR` (`~/.config/bitwig-mcp-rs/instruments`);
for the OSC backend `--send-port` (8000), `--listen-port` (9000), `--resolution` (128),
`--bank-size` (8). Logs go to stderr (`RUST_LOG=debug`).

### What the model can do

* **Read:** `get_project`, `get_track`, `get_device`, `read_notes`, `status`; resources
  `bitwig://project`, `bitwig://instruments`, `bitwig://protocol`.
* **Build:** `create_track` (kind, name, color, position and a device or preset in one call),
  `create_scene`, `create_clip` (with exact notes: `{"note": "C3", "start": 0, "length": 0.5}`),
  `write_notes`, `insert_device`.
* **Mix and play:** `set_track` (volume or `volume_db`, pan, mute, solo, arm, sends),
  `set_device_param` (by parameter name), `transport`, `set_transport`, `clip_action`,
  `scene_action`.
* **Everything else:** `list_actions` + `invoke_action` reach every Bitwig menu command
  (save, export, group, consolidate, …); `history` undoes.
* **Prompts:** `sketch_song`, `mix_check`, `setup_hardware_synth`.

### Hardware synths

Put Bitwig's **HW Instrument** device on a track (MIDI out = your synth's port, audio in =
the inputs it is plugged into). Then `play_notes`, `send_midi`, `hardware_program_change` and
`hardware_set` on that track reach the synth through Bitwig, which keeps owning the MIDI port.
`hardware_set` maps parameter names to CC / 14-bit CC / NRPN / pitch bend / aftertouch via
instrument profiles from the [`synthazia-hardware`](https://crates.io/crates/synthazia-hardware)
crate (shared with the Synthazia DAW). Built in: a General MIDI one and
the Behringer **Model D** (ch 1), **Crave** (2), **CAT** (3), **WASP Deluxe** (4), **Edge** (5)
and **Solina** (6), taken from their manuals' MIDI charts. These are analog: only the Crave has
real CCs (attack, decay, sustain, glide, tempo); the others take notes, pitch bend and, on the
Model D and Wasp, the mod wheel. Set each synth and its HW Instrument device to the profile's
channel (or change the channel in your own copy). Your own TOML files go in
`~/.config/bitwig-mcp-rs/instruments/` and replace a built-in profile of the same name
(format in the `synthazia-hardware` docs).

SysEx can't travel through a track. For SysEx parameters (all Model D and Wasp settings: MIDI
channel, key priority, bend range, mod curve, …) and the raw `send_sysex` tool, assign the
synth's MIDI port as the **MIDI output of the MCP bridge controller** in Bitwig's settings.
`play_notes` takes an `instrument` to resolve its named keys (`"kick"` from a drum map).

### Limits

The bridge sees the first 64 main tracks, 16 effect tracks, 64 scenes and 16 devices per
track. Clips are launcher clips (the Bitwig API cannot place clips in the arrangement; record
the launcher into the arranger). Editing a clip or device moves Bitwig's selection to it.
The JSON protocol between server and extension is in `bitwig-bridge/protocol.schema.json`
(generated from `src/bridge/protocol.rs`; `just schema`).

Not affiliated with Bitwig GmbH.

## Development

```sh
just              # list tasks
just bridge-install   # build + install the Bitwig extension (JDK 17+)
just test-live    # end-to-end against a running Bitwig with the bridge added
just check-all    # the quality gate: fmt, clippy, tests, versions, nu tests
just fuzz-all     # every cargo-fuzz target (nightly)
just audit deny   # supply chain
just update-deps  # cargo update (+ fuzz/) → gate → commit → push
just release 0.2.0       # bump, changelog, tag, push → Release workflow
just release-all 0.2.0   # … to GitHub and Gitea
```

Layout:

```
crates/bitwig-mcp-rs/   the server
  src/bridge/           protocol (wire types, framing), client (TCP), project (name
                        resolution), server (MCP tools, resources, prompts)
  src/osc/              DrivenByMoss backend
  src/music.rs …        pure helpers: dB, note names, note grid
  tests/                end-to-end over real stdio; live.rs against Bitwig (ignored)
bitwig-bridge/          the Bitwig extension (Java 17, no dependencies, built by
                        scripts/bridge.nu) + protocol.schema.json
fuzz/                   cargo-fuzz targets: OSC decoding, bridge framing
scripts/                all tooling, Nushell only, each with tests in scripts/tests/
```

CI runs on GitHub **and** Gitea (a nu test keeps the two in sync): quality gate,
tests on macOS and Windows, static musl build, MSRV, and three nightlies — lockfile
update + gate + push (GitHub 02:00, Gitea 02:30 UTC), `cargo audit` + `cargo deny`
(03:00), fuzzing every target (04:00). Dependabot keeps the GitHub actions current.
Commits follow [Conventional Commits](https://www.conventionalcommits.org);
`CHANGELOG.md` is generated by [git-cliff](https://git-cliff.org) — don't edit it by hand.

## License

MIT
