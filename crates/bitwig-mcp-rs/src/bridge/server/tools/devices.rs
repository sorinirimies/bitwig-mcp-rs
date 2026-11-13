//! Devices: loading, chains, parameters, drum kits.

use super::super::*;

/// A device parameter to set as part of several.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ParamSetting {
    /// Remote-control page by name or 1-based number; default: the call's page, else current.
    pub page: Option<Ref>,
    /// Parameter name or 1-based number (1-8) on the page.
    pub param: Ref,
    /// Value 0.0-1.0.
    pub value: f64,
}

/// Several device parameters at once.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetParamsArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// Default page for settings that give none.
    pub page: Option<Ref>,
    /// The parameters to set, in order.
    pub params: Vec<ParamSetting>,
}

/// One device of a chain.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChainDevice {
    /// Bitwig device or plugin name, e.g. "EQ-5", "Compressor".
    pub name: Option<String>,
    /// Preset name instead of a bare device.
    pub preset: Option<String>,
    /// Parameters to set after loading (by name or number, optionally per page).
    #[serde(default)]
    pub params: Vec<ParamSetting>,
}

/// A chain of devices on a track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertChainArgs {
    /// Track name or 1-based number; "master"; "fx N".
    pub track: Ref,
    /// The devices, loaded in this order at the end of the chain.
    pub devices: Vec<ChainDevice>,
}

/// The master chain.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct MasterChainArgs {
    /// Devices instead of the default [EQ-5, Compressor, Peak Limiter].
    pub devices: Option<Vec<ChainDevice>>,
}

/// Where a device goes.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// End of the chain.
    #[default]
    End,
    /// Start of the chain.
    Start,
    /// After `relative_to`.
    After,
    /// Before `relative_to`.
    Before,
    /// Replacing `relative_to`.
    Replace,
}

/// A device to insert.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertDeviceArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Bitwig device or plugin name as shown in the browser, e.g. "Polymer", "Compressor+".
    pub name: Option<String>,
    /// Preset name to load instead (searched in the Presets tab).
    pub preset: Option<String>,
    /// CLAP plugin id, e.g. "org.surge-synth-team.surge-xt".
    pub clap_id: Option<String>,
    /// VST3 plugin id.
    pub vst3_id: Option<String>,
    /// Absolute path of a preset/sample file.
    pub file: Option<String>,
    /// end (default), start, after, before or replace.
    pub at: Option<Placement>,
    /// Device (name or 1-based number) for after/before/replace.
    pub relative_to: Option<Ref>,
}

/// A device.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeviceArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position in the chain.
    pub device: Ref,
    /// Remote-control page by name or 1-based number; default: current page.
    pub page: Option<Ref>,
}

/// A device parameter.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetParamArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// Remote-control page by name or 1-based number; default: current page.
    pub page: Option<Ref>,
    /// Parameter name or 1-based number (1-8) on the page.
    pub param: Ref,
    /// Value 0.0-1.0.
    pub value: f64,
}

/// Device action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeviceActionArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// enable, disable, toggle, delete, duplicate, open_window, close_window, next_preset,
    /// previous_preset or select.
    pub action: DeviceAction,
}

/// A drum kit.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DrumKitArgs {
    /// Track with the Drum Machine.
    pub track: Ref,
    /// The Drum Machine device (name or 1-based position; default "Drum Machine").
    pub device: Option<Ref>,
    /// Pad → device to load, e.g. {"kick": "v8 Kick", "snare": "v8 Snare", "clap": "v8 Clap",
    /// "closed_hat": "v8 Hat", "open_hat": "v8 Hat"}. Pads are General MIDI key names, note
    /// names ("C1") or numbers.
    #[serde(default)]
    pub pads: std::collections::BTreeMap<String, String>,
    /// Effects added after a pad's instrument, in order, e.g. {"snare": ["Gate", "Reverb"]} for
    /// the gated 80s snare. Works on pads that already have an instrument (also from an earlier
    /// call).
    #[serde(default)]
    pub effects: std::collections::BTreeMap<String, Vec<String>>,
}

/// Device listing.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListDevicesArgs {
    /// Only names containing this (case-insensitive); also lists matching presets, e.g.
    /// "pad", "bass", "80s".
    pub query: Option<String>,
    /// Browser tab: "Devices" (default) or "Presets".
    pub content_type: Option<String>,
    /// At most this many (default 2000).
    pub max: Option<u32>,
}

#[tool_router(router = devices_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Insert a device into a track's chain: a Bitwig device or plugin by name, a preset by name, a CLAP/VST3 plugin by id, or a file. Placement: end (default), start, or after/before/replace relative_to a device."
    )]
    pub(crate) async fn insert_device(
        &self,
        Parameters(a): Parameters<InsertDeviceArgs>,
    ) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let rel = || -> Result<u32, McpError> {
            let r = a
                .relative_to
                .as_ref()
                .ok_or_else(|| bad("after/before/replace need relative_to"))?;
            view.device(track, r).map_err(bad)
        };
        let at = match a.at.unwrap_or_default() {
            Placement::End => Insert::End,
            Placement::Start => Insert::Start,
            Placement::After => Insert::After(rel()?),
            Placement::Before => Insert::Before(rel()?),
            Placement::Replace => Insert::Replace(rel()?),
        };
        let source = match (a.name, a.preset, a.clap_id, a.vst3_id, a.file) {
            (Some(name), None, None, None, None) => DeviceSource::Browser {
                name,
                content_type: None,
            },
            (None, Some(name), None, None, None) => DeviceSource::Browser {
                name,
                content_type: None,
            },
            (None, None, Some(id), None, None) => DeviceSource::Clap { id },
            (None, None, None, Some(id), None) => DeviceSource::Vst3 { id },
            (None, None, None, None, Some(path)) => DeviceSource::File { path },
            _ => {
                return Err(bad(
                    "give exactly one of name, preset, clap_id, vst3_id, file",
                ));
            }
        };
        out(&self.insert(track, at, source).await?)
    }

    #[tool(
        description = "A device: enabled state, remote-control page names, and the 8 parameters of a page (name, value 0-1, display text)."
    )]
    pub(crate) async fn get_device(&self, Parameters(a): Parameters<DeviceArgs>) -> ToolResult {
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        let page = self.page_index(track, device, a.page.as_ref()).await?;
        out(&self
            .call(Command::GetDevice {
                track,
                device,
                page,
            })
            .await?)
    }

    #[tool(
        description = "Set a device parameter (by name or 1-8 on a remote-control page) to 0.0-1.0. Replies with the value Bitwig shows, e.g. \"1.20 kHz\"."
    )]
    pub(crate) async fn set_device_param(
        &self,
        Parameters(a): Parameters<SetParamArgs>,
    ) -> ToolResult {
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        out(&self
            .set_param(track, device, a.page.as_ref(), &a.param, a.value)
            .await?)
    }

    #[tool(
        description = "Set several device parameters in one call, by name or number, across remote-control pages (e.g. a darker sound: cutoff, resonance, release). Replies with what Bitwig shows for each."
    )]
    pub(crate) async fn set_device_params(
        &self,
        Parameters(a): Parameters<SetParamsArgs>,
    ) -> ToolResult {
        if a.params.is_empty() {
            return Err(bad("params is empty"));
        }
        if let Some(p) = a.params.iter().find(|p| !(0.0..=1.0).contains(&p.value)) {
            return Err(bad(format!("value {} must be 0.0-1.0", p.value)));
        }
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        let mut results = Vec::new();
        for p in &a.params {
            let page = p.page.as_ref().or(a.page.as_ref());
            results.push(
                self.set_param(track, device, page, &p.param, p.value)
                    .await?,
            );
        }
        out(&Json::Array(results))
    }

    #[tool(
        description = "Load a chain of devices onto a track in one call, in order, each with optional parameters (by name), e.g. a master chain or a vocal chain. Reports every device; a failing device is reported and the rest still load."
    )]
    pub(crate) async fn insert_chain(
        &self,
        Parameters(a): Parameters<InsertChainArgs>,
    ) -> ToolResult {
        if a.devices.is_empty() {
            return Err(bad("devices is empty"));
        }
        out(&self.load_chain(&a.track, &a.devices).await?)
    }

    #[tool(
        description = "Put a mastering chain on the master track: EQ-5, Compressor and Peak Limiter by default (or your devices). Then measure with capture_audio (LUFS, true peak), adjust with set_device_params, and measure again."
    )]
    pub(crate) async fn master_chain(
        &self,
        Parameters(a): Parameters<MasterChainArgs>,
    ) -> ToolResult {
        let devices = a.devices.unwrap_or_else(|| {
            ["EQ-5", "Compressor", "Peak Limiter"]
                .into_iter()
                .map(|n| ChainDevice {
                    name: Some(n.into()),
                    preset: None,
                    params: Vec::new(),
                })
                .collect()
        });
        out(&self
            .load_chain(&Ref::Name("master".into()), &devices)
            .await?)
    }

    #[tool(
        description = "Device action: enable, disable, toggle, delete, duplicate, open_window, close_window, next_preset, previous_preset, select."
    )]
    pub(crate) async fn device_action(
        &self,
        Parameters(a): Parameters<DeviceActionArgs>,
    ) -> ToolResult {
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        out(&self
            .call(Command::DeviceAction {
                track,
                device,
                action: a.action,
            })
            .await?)
    }

    #[tool(
        description = "List what Bitwig has installed: every device (or preset) in the browser, each Bitwig device annotated with its role and sound tips from the built-in guide. Use it before choosing instruments and effects."
    )]
    pub(crate) async fn list_devices(
        &self,
        Parameters(a): Parameters<ListDevicesArgs>,
    ) -> ToolResult {
        // the browser offers what fits the track: instruments only on tracks that can hold them
        let view = self.view().await?;
        let track = view.tracks.first().map(TrackView::addr).ok_or_else(|| {
            bad("create a track first: Bitwig lists instruments only for a track that can hold them")
        })?;
        let r = self
            .call_long(Command::ListBrowser {
                track,
                content_type: a.content_type.clone().unwrap_or_else(|| "Devices".into()),
                max: a.max.unwrap_or(2000).clamp(1, 2000),
            })
            .await?;
        let needle = a.query.as_deref().map(str::to_lowercase);
        let items: Vec<Json> = r["names"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(Json::as_str)
                    .filter(|n| needle.as_ref().is_none_or(|q| n.to_lowercase().contains(q)))
                    .map(|n| match crate::devices::find(n) {
                        Some(g) => {
                            json!({"name": n, "kind": g.kind, "role": g.role, "tips": g.tips})
                        }
                        None => json!({"name": n}),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Bitwig's own devices first, then everything else (presets, plugins)
        let (mut known, other): (Vec<Json>, Vec<Json>) =
            items.into_iter().partition(|i| i.get("kind").is_some());
        let other_names: Vec<Json> = other.into_iter().map(|i| i["name"].clone()).collect();
        // presets run into the thousands: names only when searched for
        let presets = if needle.is_some() {
            json!(other_names)
        } else {
            json!(format!(
                "{} presets and plugins; pass query to search them",
                other_names.len()
            ))
        };
        known.sort_by_key(|i| i["kind"].as_str().unwrap_or_default().to_owned());
        out(&json!({
            "total_in_browser": r["total"],
            "bitwig_devices": known,
            "presets_and_plugins": presets,
        }))
    }

    #[tool(
        description = "Fill a Drum Machine's pads with instruments and add effects on single pads (effects {\"snare\": [\"Gate\", \"Reverb\"]}: the gated 80s snare), e.g. pads {\"kick\": \"v8 Kick\", \"snare\": \"v8 Snare\", \"closed_hat\": \"v8 Hat\"}. Pad names are the General MIDI keys write_drums uses, so a kit built here plays the drum patterns written there."
    )]
    pub(crate) async fn build_drum_kit(
        &self,
        Parameters(a): Parameters<DrumKitArgs>,
    ) -> ToolResult {
        if (a.pads.is_empty() && a.effects.is_empty()) || a.pads.len() > 32 {
            return Err(bad("give 1-32 pads and/or effects"));
        }
        let dref = a.device.unwrap_or_else(|| Ref::Name("Drum Machine".into()));
        let (track, device) = self.track_device(&a.track, &dref).await?;
        let registry = self.profiles();
        let gm = registry.get("generic");
        let mut plan = Vec::new();
        for (pad, name) in &a.pads {
            plan.push((voice_key(gm, pad)?, pad.clone(), name.clone()));
        }
        let mut done = Vec::new();
        for (key, pad, name) in plan {
            match self
                .call_long(Command::DrumPadInsert {
                    track,
                    device,
                    key,
                    source: DeviceSource::Browser {
                        name: name.clone(),
                        content_type: None,
                    },
                    append: false,
                })
                .await
            {
                Ok(_) => done.push(json!({"pad": pad, "key": key, "device": name})),
                Err(e) => done.push(json!({"pad": pad, "key": key, "error": e.message})),
            }
        }
        let mut fx_done = Vec::new();
        for (pad, names) in &a.effects {
            let key = voice_key(gm, pad)?;
            for name in names {
                match self
                    .call_long(Command::DrumPadInsert {
                        track,
                        device,
                        key,
                        source: DeviceSource::Browser {
                            name: name.clone(),
                            content_type: None,
                        },
                        append: true,
                    })
                    .await
                {
                    Ok(_) => fx_done.push(json!({"pad": pad, "effect": name})),
                    Err(e) => fx_done.push(json!({"pad": pad, "effect": name, "error": e.message})),
                }
            }
        }
        // the cursor device moves into the pad it filled: ask for the drum machine itself
        let machine = self
            .call(Command::GetDevice {
                track,
                device,
                page: None,
            })
            .await
            .map(|d| d["drum_pads"].clone())
            .unwrap_or(Json::Null);
        out(&json!({"pads": done, "effects": fx_done, "drum_pads_now": machine}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn insert_chain_loads_devices_in_order_and_sets_params() {
        let (s, log) = rig().await;
        let r = s
            .insert_chain(Parameters(InsertChainArgs {
                track: Ref::Name("synth".into()),
                devices: vec![
                    ChainDevice {
                        name: Some("EQ-5".into()),
                        preset: None,
                        params: Vec::new(),
                    },
                    ChainDevice {
                        name: Some("Peak Limiter".into()),
                        preset: None,
                        params: vec![ParamSetting {
                            page: None,
                            param: Ref::Number(1),
                            value: 0.5,
                        }],
                    },
                ],
            }))
            .await
            .expect("tool");
        assert!(!r.contains("error"), "{r}");
        let ins = sent(&log, "insert_device");
        assert_eq!(ins.len(), 2);
        assert_eq!(ins[0]["source"]["name"], "EQ-5");
        assert_eq!(ins[1]["source"]["name"], "Peak Limiter");
        assert_eq!(sent(&log, "set_device_param").len(), 1);
    }

    #[tokio::test]
    async fn master_chain_defaults_to_eq_compressor_limiter() {
        let (s, log) = rig().await;
        s.master_chain(Parameters(MasterChainArgs::default()))
            .await
            .expect("tool");
        let names: Vec<_> = sent(&log, "insert_device")
            .iter()
            .map(|r| r["source"]["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(names, ["EQ-5", "Compressor", "Peak Limiter"]);
    }

    #[tokio::test]
    async fn drum_kit_effects_append_to_the_pad() {
        let (s, log) = rig().await;
        let mut effects = std::collections::BTreeMap::new();
        effects.insert(
            "snare".to_owned(),
            vec!["Gate".to_owned(), "Reverb".to_owned()],
        );
        s.build_drum_kit(Parameters(DrumKitArgs {
            track: Ref::Name("drums".into()),
            device: None,
            pads: std::collections::BTreeMap::new(),
            effects,
        }))
        .await
        .expect("tool");
        let sent = sent(&log, "drum_pad_insert");
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|r| r["append"] == true && r["key"] == 38));
        assert_eq!(sent[1]["source"]["name"], "Reverb");
    }

    #[tokio::test]
    async fn device_params_resolve_by_page_and_name() {
        let (s, log) = rig().await;
        s.set_device_param(Parameters(SetParamArgs {
            track: Ref::Name("drums".into()),
            device: Ref::Name("drum".into()),
            page: Some(Ref::Name("filter".into())),
            param: Ref::Name("reso".into()),
            value: 0.3,
        }))
        .await
        .expect("tool");
        let req = &sent(&log, "set_device_param")[0];
        assert_eq!(
            (req["page"].clone(), req["param"].clone()),
            (json!(1), json!(7)),
            "the parameter's own index, not its place in the list"
        );
    }
}
