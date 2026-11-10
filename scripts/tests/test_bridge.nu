#!/usr/bin/env nu
# ── Bitwig MCP · test_bridge.nu ───────────────────────────────────────────────

use std/assert
use runner.nu *
use ../bridge.nu [api_url extensions_dir build_info schema_protocol asset_name missing_javac API_VERSION API_SHA256]

def "test bridge: api url points at the Bitwig maven" [] {
    assert equal (api_url 25) "https://maven.bitwig.com/com/bitwig/extension-api/25/extension-api-25.jar"
    assert equal ($API_SHA256 | str length) 64
    assert ($API_VERSION >= 18)
}

def "test bridge: extension folders per OS" [] {
    assert equal (extensions_dir "linux" "/home/u") "/home/u/Bitwig Studio/Extensions"
    assert equal (extensions_dir "macos" "/Users/u") "/Users/u/Documents/Bitwig Studio/Extensions"
    assert (extensions_dir "windows" "/c/u" | str ends-with "Extensions")
    assert error { extensions_dir "plan9" "/u" }
}

def "test bridge: build info carries version and protocol" [] {
    let src = (build_info "1.2.3" 4)
    assert ($src | str contains 'VERSION = "1.2.3";')
    assert ($src | str contains 'PROTOCOL = 4;')
    assert ($src | str contains "package io.github.sorinirimies.bitwigmcp;")
    assert error { build_info "x" 1 }
}

def "test bridge: protocol version comes from the schema" [] {
    assert equal (schema_protocol {protocol_version: 1}) 1
    assert error { schema_protocol {} }
    let root = ($env.CURRENT_FILE | path dirname | path join ".." "..")
    let schema = (open ($root | path join "bitwig-bridge" "protocol.schema.json"))
    assert ((schema_protocol $schema) >= 1)
}

def "test bridge: release asset name carries the version" [] {
    assert equal (asset_name "1.2.3") "bitwig-mcp-rs-bridge-1.2.3.bwextension"
}

def "test bridge: CI fails without javac, developers skip" [] {
    assert equal (missing_javac true) "fail"
    assert equal (missing_javac false) "skip"
}

def main [] { run-tests }
