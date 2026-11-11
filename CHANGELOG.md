# Changelog

All notable changes to this project are documented here. It is generated from
[Conventional Commits](https://www.conventionalcommits.org) by
[git-cliff](https://git-cliff.org) — do not edit it by hand.

## [0.1.1](https://github.com/sorinirimies/bitwig-mcp-rs/compare/v0.1.0...v0.1.1) — 2026-11-11

### ✨ Features

- **extension:** Bundle the Bitwig extension in the binary and install it ([`be913cf`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/be913cfc08c9fd71c37e2bb27d75f9f31bb01fbe))

### 🐛 Bug fixes

- **bridge:** Name the controller as Bitwig lists it in the connection error ([`8bc203d`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/8bc203d389f3184dcab0fbce1b6c53da15bffc78))

### 🔄 CI and release

- **secrets:** Ignore the extension API jar checksum, a gitleaks false positive ([`1d7f65f`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/1d7f65fc35c48c23fc3785a86cdabb91619cf35c))

## [0.1.0](https://github.com/sorinirimies/bitwig-mcp-rs/releases/tag/v0.1.0) — 2026-11-11

### ✨ Features

- **hardware:** Use the published synthazia-hardware crate, add SysEx ([`7285ec0`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/7285ec0c10c169b8d9c173e3fcd687a65be86b83))
- **hardware:** Built-in profiles for Behringer Model D, Crave, CAT, Wasp, Edge, Solina ([`b1a1880`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/b1a188078e939432c2dea0b0e8efe9ccab0e6e6e))
- **bridge:** Own Bitwig extension with a JSON/TCP protocol and a name-based tool set ([`a35f1a4`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/a35f1a4af9923e2b03f8806d05068655686c2536))
- MCP server for Bitwig Studio over DrivenByMoss OSC ([`f49393a`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/f49393ad40e5769f47f833369c222d17d8877ce8))
- Initial project scaffold ([`20df4de`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/20df4de3cc902d27fc1effdfc675d7dbfc5e00dc))

### 🔄 CI and release

- **gitea:** Publish the crate to crates.io from the Gitea release ([`8b226ee`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/8b226ee341433990b5b1feed9ae9aff1a61bf052))
- Bump softprops/action-gh-release from 2 to 3 (#2) ([`25b5451`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/25b5451942150880ff984cecf77f9e943eb3041c))
- Build the Bitwig extension in CI and ship it with every release ([`9eaaaba`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/9eaaaba11f581812860cf519ec88b94fe2d8921d))


