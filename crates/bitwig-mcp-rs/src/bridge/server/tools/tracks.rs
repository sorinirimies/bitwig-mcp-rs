//! Tracks, scenes, returns.

use super::super::*;

/// A send from one track to a new effect track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReturnSend {
    /// Source track name or 1-based number.
    pub track: Ref,
    /// Send level in dB.
    pub volume_db: f64,
}

/// A new effect (return) track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateReturnArgs {
    /// Name of the return, e.g. "Reverb"; sends use it.
    pub name: String,
    /// Devices of the chain, in order, e.g. [{"name": "Reverb", "params": [...]}].
    #[serde(default)]
    pub devices: Vec<ChainDevice>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// Tracks to send into it, with levels.
    #[serde(default)]
    pub sends: Vec<ReturnSend>,
}

/// One send level.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendArg {
    /// Send by effect-track name or 1-based number.
    pub send: Ref,
    /// Level 0.0-1.0.
    pub volume: Option<f64>,
    /// Level in dB (wins over volume).
    pub volume_db: Option<f64>,
}

/// Track settings; only given fields change.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetTrackArgs {
    /// Track name or 1-based number; "master"; "fx N".
    pub track: Ref,
    /// New name.
    pub name: Option<String>,
    /// "#rrggbb" or a name (red, orange, yellow, green, teal, blue, purple, pink, ...).
    pub color: Option<String>,
    /// Volume 0.0-1.0 (0.79 ≈ 0 dB).
    pub volume: Option<f64>,
    /// Volume in dB, e.g. -6 (wins over volume).
    pub volume_db: Option<f64>,
    /// Pan 0.0 (left) - 1.0 (right), 0.5 = center.
    pub pan: Option<f64>,
    /// Mute.
    pub mute: Option<bool>,
    /// Solo.
    pub solo: Option<bool>,
    /// Record arm.
    pub arm: Option<bool>,
    /// Activated (deactivated tracks use no CPU).
    pub activated: Option<bool>,
    /// Input monitoring: "on", "off" or "auto".
    pub monitor: Option<String>,
    /// Send levels.
    #[serde(default)]
    pub sends: Vec<SendArg>,
}

/// A new track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTrackArgs {
    /// audio, instrument or effect.
    pub kind: TrackKind,
    /// Name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// 1-based position among main tracks; default: at the end.
    pub position: Option<u32>,
    /// Device to load by name, e.g. "Polymer", "Drum Machine", "HW Instrument".
    pub device: Option<String>,
    /// Preset to load by name instead of a bare device.
    pub preset: Option<String>,
}

/// Track action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackActionArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// select, duplicate, delete, stop_clips, return_to_arrangement or show.
    pub action: TrackAction,
}

/// A new scene.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSceneArgs {
    /// Name, e.g. "Verse".
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
}

/// Scene settings.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetSceneArgs {
    /// Scene name or 1-based number.
    pub scene: Ref,
    /// New name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
}

/// Scene action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SceneActionArgs {
    /// Scene name or 1-based number.
    pub scene: Ref,
    /// launch, select, duplicate or delete.
    pub action: SceneAction,
}

#[tool_router(router = tracks_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Create a track (audio, instrument or effect) and set it up in one go: name, color, position and a device or preset loaded by name (e.g. device \"Polymer\", \"Drum Machine\", \"HW Instrument\"). Replies with the new track."
    )]
    pub(crate) async fn create_track(
        &self,
        Parameters(a): Parameters<CreateTrackArgs>,
    ) -> ToolResult {
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        let position = match a.position {
            Some(0) => return Err(bad("position starts at 1")),
            Some(p) => i32::try_from(p - 1).map_err(err)?,
            None => -1,
        };
        let created = self
            .call(Command::CreateTrack {
                kind: a.kind,
                position,
            })
            .await?;
        check_created_type(a.kind, created["type"].as_str()).map_err(err)?;
        let track: TrackAddr = serde_json::from_value(created).map_err(err)?;
        let mut notes = Vec::new();
        let source = match (a.device, a.preset) {
            (_, Some(p)) => Some(DeviceSource::Browser {
                name: p,
                content_type: None,
            }),
            (Some(d), None) => Some(DeviceSource::Browser {
                name: d,
                content_type: None,
            }),
            (None, None) => None,
        };
        if let Some(source) = source
            && let Err(e) = self.insert(track, Insert::End, source).await
        {
            notes.push(format!("device not loaded: {}", e.message));
        }
        let state = self
            .set_track_change(
                track,
                TrackChange {
                    name: a.name,
                    color,
                    ..TrackChange::default()
                },
            )
            .await?;
        out(&json!({ "track": state, "warnings": notes }))
    }

    #[tool(
        description = "Change a track: name, color, volume (0-1 or volume_db), pan, mute, solo, arm, activated, monitor, send levels. Only given fields change. Replies with the track after the change, including Bitwig's display text (e.g. \"-6.0 dB\")."
    )]
    pub(crate) async fn set_track(&self, Parameters(a): Parameters<SetTrackArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let mut sends = Vec::new();
        for s in &a.sends {
            let index = view.send(track, &s.send).map_err(bad)?;
            let volume = send_volume_of(s.volume, s.volume_db)?
                .ok_or_else(|| bad("each send needs volume or volume_db"))?;
            sends.push(SendLevel { index, volume });
        }
        let monitor_mode = match a.monitor.as_deref().map(str::to_lowercase).as_deref() {
            None => None,
            Some("on") => Some("ON".to_owned()),
            Some("off") => Some("OFF".to_owned()),
            Some("auto") => Some("AUTO".to_owned()),
            Some(o) => return Err(bad(format!("monitor must be on, off or auto, not {o:?}"))),
        };
        let change = TrackChange {
            name: a.name,
            color: a
                .color
                .as_deref()
                .map(parse_color)
                .transpose()
                .map_err(bad)?,
            volume: volume_of(a.volume, a.volume_db)?,
            pan: unit("pan", a.pan)?,
            mute: a.mute,
            solo: a.solo,
            arm: a.arm,
            activated: a.activated,
            monitor_mode,
            sends,
        };
        out(&self.set_track_change(track, change).await?)
    }

    #[tool(
        description = "Track action: select, duplicate, delete, stop_clips, return_to_arrangement, show. delete is undoable with history."
    )]
    pub(crate) async fn track_action(
        &self,
        Parameters(a): Parameters<TrackActionArgs>,
    ) -> ToolResult {
        let (_, track) = self.track_of(&a.track).await?;
        out(&self
            .call(Command::TrackAction {
                track,
                action: a.action,
            })
            .await?)
    }

    #[tool(
        description = "Create a scene at the end of the scene list, with a name and color. Replies with the scene number."
    )]
    pub(crate) async fn create_scene(
        &self,
        Parameters(a): Parameters<CreateSceneArgs>,
    ) -> ToolResult {
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        let r = self.call(Command::CreateScene).await?;
        let index = r["index"].as_u64().ok_or_else(|| err("no scene index"))?;
        let scene = u32::try_from(index).map_err(err)?;
        if a.name.is_some() || color.is_some() {
            self.call(Command::SetScene {
                scene,
                name: a.name.clone(),
                color,
            })
            .await?;
        }
        out(&json!({"scene": scene + 1, "name": a.name}))
    }

    #[tool(description = "Rename or recolor a scene.")]
    pub(crate) async fn set_scene(&self, Parameters(a): Parameters<SetSceneArgs>) -> ToolResult {
        let (_, scene) = self.resolve(|v| v.scene(&a.scene)).await?;
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        out(&self
            .call(Command::SetScene {
                scene,
                name: a.name,
                color,
            })
            .await?)
    }

    #[tool(description = "Scene action: launch, select, duplicate, delete.")]
    pub(crate) async fn scene_action(
        &self,
        Parameters(a): Parameters<SceneActionArgs>,
    ) -> ToolResult {
        let (_, scene) = self.resolve(|v| v.scene(&a.scene)).await?;
        out(&self
            .call(Command::SceneAction {
                scene,
                action: a.action,
            })
            .await?)
    }

    #[tool(
        description = "Create an effect (return) track with a device chain and send levels in one call, e.g. a reverb bus: name \"Reverb\", devices [{\"name\": \"Reverb\"}], sends [{track: \"Pad\", volume_db: -12}]."
    )]
    pub(crate) async fn create_return(
        &self,
        Parameters(a): Parameters<CreateReturnArgs>,
    ) -> ToolResult {
        let created = self
            .create_track(Parameters(CreateTrackArgs {
                kind: TrackKind::Effect,
                name: Some(a.name.clone()),
                color: a.color,
                position: None,
                device: None,
                preset: None,
            }))
            .await?;
        let chain = if a.devices.is_empty() {
            Json::Null
        } else {
            self.load_chain(&Ref::Name(a.name.clone()), &a.devices)
                .await?
        };
        let mut sends = Vec::new();
        for s in &a.sends {
            let r = self
                .set_track(Parameters(SetTrackArgs {
                    track: s.track.clone(),
                    name: None,
                    color: None,
                    volume: None,
                    volume_db: None,
                    pan: None,
                    mute: None,
                    solo: None,
                    arm: None,
                    activated: None,
                    monitor: None,
                    sends: vec![SendArg {
                        send: Ref::Name(a.name.clone()),
                        volume: None,
                        volume_db: Some(s.volume_db),
                    }],
                }))
                .await;
            sends.push(match r {
                Ok(_) => json!({"track": s.track, "volume_db": s.volume_db, "ok": true}),
                Err(e) => json!({"track": s.track, "error": e.message}),
            });
        }
        out(&json!({
            "created": serde_json::from_str::<Json>(&created).unwrap_or(Json::Null),
            "chain": chain,
            "sends": sends,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn create_return_makes_an_effect_track_and_sets_the_sends() {
        let (s, log) = rig().await;
        let r = s
            .create_return(Parameters(CreateReturnArgs {
                name: "Reverb".into(),
                devices: vec![ChainDevice {
                    name: Some("Reverb".into()),
                    preset: None,
                    params: Vec::new(),
                }],
                color: None,
                sends: vec![ReturnSend {
                    track: Ref::Name("drums".into()),
                    volume_db: -12.0,
                }],
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"ok\":true"), "{r}");
        assert_eq!(sent(&log, "create_track")[0]["kind"], "effect");
        assert_eq!(sent(&log, "insert_device").len(), 1);
        assert!(
            sent(&log, "set_track")
                .iter()
                .any(|r| r["sends"][0]["volume"].is_number()),
        );
    }

    #[tokio::test]
    async fn set_track_resolves_names_and_converts_db() {
        let (s, log) = rig().await;
        s.set_track(Parameters(SetTrackArgs {
            track: Ref::Name("drums".into()),
            name: None,
            color: Some("red".into()),
            volume: None,
            volume_db: Some(-6.0),
            pan: Some(0.25),
            mute: Some(false),
            solo: None,
            arm: None,
            activated: None,
            monitor: Some("Auto".into()),
            sends: vec![SendArg {
                send: Ref::Name("reverb".into()),
                volume: None,
                volume_db: Some(0.0),
            }],
        }))
        .await
        .expect("tool");
        let req = &sent(&log, "set_track")[0];
        assert_eq!(req["track"], json!({"bank": "main", "index": 0}));
        let v = req["volume"].as_f64().expect("volume");
        assert!((music::volume_to_db(v) + 6.0).abs() < 1e-9);
        assert_eq!(req["color"], json!([217, 46, 36]));
        assert_eq!(req["monitor_mode"], "AUTO");
        assert_eq!(req["sends"][0]["index"], 0);
        assert!(req.get("solo").is_none(), "absent fields are not sent");
    }

    #[tokio::test]
    async fn create_track_loads_a_device_and_names_it() {
        let (s, log) = rig().await;
        let r = s
            .create_track(Parameters(CreateTrackArgs {
                kind: TrackKind::Instrument,
                name: Some("Bass".into()),
                color: Some("#0000ff".into()),
                position: None,
                device: Some("Polymer".into()),
                preset: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"warnings\":[]"), "{r}");
        assert_eq!(sent(&log, "create_track")[0]["position"], -1);
        let ins = &sent(&log, "insert_device")[0];
        assert_eq!(ins["track"]["index"], 2);
        assert_eq!(ins["source"]["name"], "Polymer");
        let set = &sent(&log, "set_track")[0];
        assert_eq!(set["name"], "Bass");
    }

    #[tokio::test]
    async fn create_track_reports_a_missing_device_as_a_warning() {
        let (s, _) = rig().await;
        let r = s
            .create_track(Parameters(CreateTrackArgs {
                kind: TrackKind::Instrument,
                name: None,
                color: None,
                position: Some(1),
                device: Some("Nope".into()),
                preset: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("not found"), "{r}");
    }

    #[tokio::test]
    async fn create_scene_names_the_new_scene() {
        let (s, log) = rig().await;
        let r = s
            .create_scene(Parameters(CreateSceneArgs {
                name: Some("Chorus".into()),
                color: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"scene\":3"), "{r}");
        assert_eq!(sent(&log, "set_scene")[0]["scene"], 2);
    }
}
