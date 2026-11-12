# Changelog

All notable changes to this project are documented here. It is generated from
[Conventional Commits](https://www.conventionalcommits.org) by
[git-cliff](https://git-cliff.org) — do not edit it by hand.

## [0.2.2](https://github.com/sorinirimies/bitwig-mcp-rs/compare/v0.2.1...v0.2.2) — 2026-11-12

### 📚 Documentation

- README lists every tool group, the arrangement flow and the bump step ([`10dfd98`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/10dfd98d0ae23735d16f17129db6de0ff02f6e55))

## [0.2.1](https://github.com/sorinirimies/bitwig-mcp-rs/compare/v0.2.0...v0.2.1) — 2026-11-12

### 🧪 Tests

- Live test for composing, chains, meters and recording ([`d009ed5`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/d009ed5849e10df507298b46a1d393309a0f0744))

## [0.2.0](https://github.com/sorinirimies/bitwig-mcp-rs/compare/v0.1.2...v0.2.0) — 2026-11-12

### ✨ Features

- **bridge:** Replies to set_track, set_scene and set_device_param wait until Bitwig shows the change ([`b4b22dc`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/b4b22dc551d2fb96f33968558ee0f4be762d8cbf))
- Record_arrangement, record_automation; tool cap 60; docs ([`b8a6aab`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/b8a6aab47a8558a93355942935f7d113ffa09370))
- Get_meters, effects on single drum pads (extension rebuilt) ([`98e9780`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/98e978041fe40938489f2f24af2101ee947bcd05))
- Write_melody, insert_chain, create_return, master_chain ([`e5f4d91`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/e5f4d91c9e9920b99ef402d47a1417a92fd22f94))
- Copy_clip (varied copies into other scenes) and build_song (sections of the song plan) ([`631ccbd`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/631ccbd987a936bbd69fda64f84a969604df50f1))
- Get_project summary by default (detail:true for all); set_device_params ([`23fb1d0`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/23fb1d027b74b96e75ada57c444b87beca7edf71))
- Status reports server_version; bump rebuilds the bundled extension ([`47d9a88`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/47d9a88ea5606d6b86f87c3d84e193259565fa41))

### 🐛 Bug fixes

- **fuzz:** Decode_osc uses the osc:: module paths ([`1d3e9f8`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/1d3e9f86e734c4ab48d59f0eb4496516b227f23c))
- Capture records the master pair at full level; params, devices, hints ([`95ef359`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/95ef359dfdfd9dbdb9e1daf4030aeae281aceaf6))

### 🔄 CI and release

- Quality gate compiles the fuzz targets ([`3b03063`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/3b03063d230422239db378342933e24ddac46c2b))

## [0.1.2](https://github.com/sorinirimies/bitwig-mcp-rs/compare/v0.1.1...v0.1.2) — 2026-11-12

### ✨ Features

- **sound:** Build_drum_kit, list_devices and a guide to Bitwig's devices ([`a5f3baa`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/a5f3baae447b413bd320799a72679fee88ba1f26))
- **export:** MIDI files, audio capture with loudness, save and export dialogs ([`3d7e47c`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/3d7e47c2ff7e27e43e00153aee26ae4fcac361b6))
- **hardware:** Synthazia-hardware 0.0.4 with the Behringer 2600 profile ([`43a2345`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/43a23452ae15fbe1f44f2f49769ad7a56e0afdd7))
- **compose:** Music theory and composing tools, record_clip, song plan ([`b7a7460`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/b7a746039a86f55c1c24b7b5a91ba382fc6ff979))

### 🐛 Bug fixes

- Sends in dB, presets by name, fresh names, full device lists, kit result ([`ceedf80`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/ceedf806edcdfa0c05c5f1236c353c8ccdc86cf7))
- **server:** One tool call at a time; Bitwig 6 preset tab ([`858afc5`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/858afc5f0f21dc5e3a1e1029731269b185942043))
- **drums:** Pads land on their key; list_devices stays small ([`b29e703`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/b29e7034ff7073339ef358425e31a724e23831c2))
- **browser:** Reset filters before searching; list devices on a normal track ([`10d7155`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/10d7155204762aea68385d849cab1902d6a27341))
- **notes:** Read_notes keeps every note across clip and grid changes ([`7d988c7`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/7d988c7c8d8c9f4b1d819cba614ea8936157cca2))
- **extension:** Own TCP server, real track lookup, values that apply ([`5432096`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/543209677c39feb36bf2db2d8c221139915c60a2))

### 📚 Documentation

- The 2600 profile in the README ([`04a946e`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/04a946eaeb67d12e50b0e95675bec995504916f1))

### 📦 Dependencies

- Nightly cargo update 2026-11-11 ([`e8d5477`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/e8d5477f28b377282b30cf323470909376ad0a35))

### 🔄 CI and release

- Bump workflow actions ([`d9f7400`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/d9f74007a628a8a90081ff93d2a086f568af0231))

### 🔧 Chores

- Align with the project rules ([`7bdf028`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/7bdf0283dcfb9429702621e5a98dd328637ad741))
- Ignore images in the repo root ([`30dd268`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/30dd2680bfe3a2686c2a5f24e1e09f2d77cb9089))

## [0.1.1](https://github.com/sorinirimies/bitwig-mcp-rs/compare/v0.1.0...v0.1.1) — 2026-11-11

### ✨ Features

- **extension:** Bundle the Bitwig extension in the binary and install it ([`a045bba`](https://github.com/sorinirimies/bitwig-mcp-rs/commit/a045bba3418005961fb6d48f529b4eb9f3a45022))

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


