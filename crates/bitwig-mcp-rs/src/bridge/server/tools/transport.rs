//! Transport, recording the scenes into the arranger, automation takes.

use super::super::*;

/// Transport action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransportArgs {
    /// What to do.
    pub action: TransportAction,
}

/// Transport settings; only given fields change.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetTransportArgs {
    /// Tempo in BPM (20-666).
    pub tempo: Option<f64>,
    /// Arranger loop on/off.
    pub r#loop: Option<bool>,
    /// Loop start in beats.
    pub loop_start: Option<f64>,
    /// Loop length in beats.
    pub loop_length: Option<f64>,
    /// Metronome on/off.
    pub metronome: Option<bool>,
    /// Play position in beats.
    pub position: Option<f64>,
    /// Like "4/4", "6/8".
    pub time_signature: Option<String>,
    /// Punch in.
    pub punch_in: Option<bool>,
    /// Punch out.
    pub punch_out: Option<bool>,
    /// Arranger overdub.
    pub arranger_overdub: Option<bool>,
    /// Launcher overdub.
    pub launcher_overdub: Option<bool>,
    /// Arranger automation write.
    pub automation_write: Option<bool>,
    /// "latch", "touch" or "write".
    pub automation_mode: Option<String>,
    /// "none", "one_bar", "two_bars" or "four_bars".
    pub pre_roll: Option<String>,
    /// Default launch quantization: "none", "8", "4", "2", "1", "1/2", "1/4", "1/8", "1/16".
    pub launch_quantization: Option<String>,
}

/// A scene of the arrangement to record.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ArrangementScene {
    /// Scene name or 1-based number.
    pub scene: Ref,
    /// How many bars it plays (default 8).
    pub bars: Option<u32>,
}

/// Record the scenes into the arranger.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct RecordArrangementArgs {
    /// The scenes in order. Default: the sections of the song plan (`scene` or `name`, `bars`);
    /// without a plan, every scene that holds clips, 8 bars each.
    pub scenes: Option<Vec<ArrangementScene>>,
    /// Arranger position in beats where the recording starts (default 0).
    pub start_beat: Option<f64>,
}

/// Record a parameter move as arranger automation.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordAutomationArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Device name or 1-based position.
    pub device: Ref,
    /// Remote-control page; default: current page.
    pub page: Option<Ref>,
    /// Parameter name or 1-based number on the page.
    pub param: Ref,
    /// Start value 0-1.
    pub from: f64,
    /// End value 0-1.
    pub to: f64,
    /// Length in bars (1-64).
    pub bars: u32,
    /// Arranger position in beats where it starts (default 0).
    pub start_beat: Option<f64>,
    /// Scene to launch at the start so the sweep is heard over the music (optional).
    pub scene: Option<Ref>,
    /// linear (default), ease_in (slow start, a riser) or ease_out.
    pub curve: Option<String>,
}

#[tool_router(router = transport_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Transport: play, stop, toggle_play, record, restart, rewind, fast_forward, tap_tempo, stop_all_clips, add_cue_marker, next_cue_marker, previous_cue_marker."
    )]
    pub(crate) async fn transport(&self, Parameters(a): Parameters<TransportArgs>) -> ToolResult {
        out(&self.call(Command::Transport { action: a.action }).await?)
    }

    #[tool(
        description = "Change tempo, loop, metronome, position, time signature, punch, overdub, automation and launch quantization. Only given fields change. Replies with the transport state."
    )]
    pub(crate) async fn set_transport(
        &self,
        Parameters(a): Parameters<SetTransportArgs>,
    ) -> ToolResult {
        if a.tempo.is_some_and(|t| !(20.0..=666.0).contains(&t)) {
            return Err(bad("tempo must be 20-666 BPM"));
        }
        if let Some(sig) = &a.time_signature {
            let ok = sig.split_once('/').is_some_and(|(n, d)| {
                n.trim().parse::<u8>().is_ok_and(|n| n > 0)
                    && d.trim().parse::<u8>().is_ok_and(|d| d.is_power_of_two())
            });
            if !ok {
                return Err(bad("time_signature must look like 4/4 or 6/8"));
            }
        }
        let change = TransportChange {
            tempo: a.tempo,
            r#loop: a.r#loop,
            loop_start: a.loop_start,
            loop_length: a.loop_length,
            metronome: a.metronome,
            position: a.position,
            time_signature: a.time_signature.map(|s| s.replace(' ', "")),
            punch_in: a.punch_in,
            punch_out: a.punch_out,
            arranger_overdub: a.arranger_overdub,
            launcher_overdub: a.launcher_overdub,
            automation_write: a.automation_write,
            automation_mode: a.automation_mode,
            pre_roll: a.pre_roll,
            launch_quantization: a.launch_quantization,
        };
        out(&self.call(Command::SetTransport { change }).await?)
    }

    #[tool(
        description = "Record the scenes, one after the other, into the arranger: the step from a loop sketch to a finished song. Launches each scene for its bars (from the song plan's sections, or given) with arranger recording on, then stops. Takes as long as the song; the arranger afterwards holds the clips. The scene launch quantization (usually one bar) keeps the cuts on the beat."
    )]
    pub(crate) async fn record_arrangement(
        &self,
        Parameters(a): Parameters<RecordArrangementArgs>,
    ) -> ToolResult {
        let (snap, view) = self.project().await?;
        let tempo = snap["transport"]["tempo"].as_f64().unwrap_or(120.0);
        let bar = beats_per_bar(
            snap["transport"]["time_signature"]
                .as_str()
                .unwrap_or("4/4"),
        );
        let secs_per_beat = 60.0 / tempo.max(20.0);
        // (scene index, name, bars)
        let mut plan: Vec<(u32, String, u32)> = Vec::new();
        match a.scenes {
            Some(list) => {
                for s in list {
                    let i = view.scene(&s.scene).map_err(bad)?;
                    let name = view
                        .scenes
                        .iter()
                        .find(|x| x.index == i)
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    plan.push((i, name, s.bars.unwrap_or(8)));
                }
            }
            None => {
                let from_plan = self.call(Command::GetSongPlan).await.ok().and_then(|r| {
                    serde_json::from_str::<Json>(r["text"].as_str().unwrap_or_default()).ok()
                });
                for sec in from_plan
                    .as_ref()
                    .and_then(|p| p["sections"].as_array())
                    .into_iter()
                    .flatten()
                {
                    let name = sec["scene"]
                        .as_str()
                        .or(sec["name"].as_str())
                        .unwrap_or_default();
                    if let Ok(i) = view.scene(&Ref::Name(name.to_owned())) {
                        plan.push((
                            i,
                            name.to_owned(),
                            sec["bars"].as_u64().map_or(8, |b| b as u32),
                        ));
                    }
                }
                if plan.is_empty() {
                    for sc in snap["scenes"].as_array().into_iter().flatten() {
                        if sc["clip_count"].as_u64().unwrap_or(0) > 0 {
                            plan.push((
                                sc["index"].as_u64().unwrap_or(0) as u32,
                                sc["name"].as_str().unwrap_or_default().to_owned(),
                                8,
                            ));
                        }
                    }
                }
            }
        }
        if plan.is_empty() {
            return Err(bad(
                "no scenes to record: create clips in scenes, or give scenes",
            ));
        }
        let total_bars: u32 = plan.iter().map(|p| p.2).sum();
        if plan.iter().any(|p| p.2 == 0) || total_bars > 512 {
            return Err(bad("bars must be 1 or more, 512 in all"));
        }
        // start clean: stop, rewind to the start position, arranger recording on
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        self.call(Command::Transport {
            action: TransportAction::StopAllClips,
        })
        .await?;
        self.call(Command::SetTransport {
            change: TransportChange {
                position: Some(a.start_beat.unwrap_or(0.0)),
                ..TransportChange::default()
            },
        })
        .await?;
        if snap["transport"]["recording"] != true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        let begin = tokio::time::Instant::now();
        let mut at_bars = 0u32;
        let mut timeline = Vec::new();
        for (i, (scene, name, bars)) in plan.iter().enumerate() {
            if i > 0 {
                // launch a little early: the launch quantization lands it on the bar line
                let when = begin
                    + Duration::from_secs_f64(f64::from(at_bars) * bar * secs_per_beat - 0.12);
                tokio::time::sleep_until(when).await;
            }
            self.call(Command::SceneAction {
                scene: *scene,
                action: SceneAction::Launch,
            })
            .await?;
            timeline.push(json!({"scene": name, "start_bar": at_bars + 1, "bars": bars}));
            at_bars += bars;
        }
        tokio::time::sleep_until(
            begin + Duration::from_secs_f64(f64::from(total_bars) * bar * secs_per_beat),
        )
        .await;
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        let after = self.call(Command::GetProject).await?;
        if after["transport"]["recording"] == true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        out(&json!({
            "recorded_bars": total_bars,
            "tempo": tempo,
            "timeline": timeline,
            "hint": "the clips are in the arranger now; switch the view with set_view layout ARRANGE, then save_project",
        }))
    }

    #[tool(
        description = "Record a parameter move (filter sweep, riser, volume swell) as arranger automation: sets the parameter from a start to an end value over some bars while the arranger records with automation write on. Optionally launches a scene so the sweep is made over the music. Takes as long as the bars; the move is written in small steps."
    )]
    pub(crate) async fn record_automation(
        &self,
        Parameters(a): Parameters<RecordAutomationArgs>,
    ) -> ToolResult {
        if !(1..=64).contains(&a.bars) {
            return Err(bad("bars must be 1-64"));
        }
        if ![a.from, a.to].iter().all(|v| (0.0..=1.0).contains(v)) {
            return Err(bad("from and to must be 0.0-1.0"));
        }
        let curve = a.curve.as_deref().unwrap_or("linear");
        if !matches!(curve, "linear" | "ease_in" | "ease_out") {
            return Err(bad("curve must be linear, ease_in or ease_out"));
        }
        let (track, device) = self.track_device(&a.track, &a.device).await?;
        let snap = self.call(Command::GetProject).await?;
        let tempo = snap["transport"]["tempo"].as_f64().unwrap_or(120.0);
        let bar = beats_per_bar(
            snap["transport"]["time_signature"]
                .as_str()
                .unwrap_or("4/4"),
        );
        let secs = f64::from(a.bars) * bar * 60.0 / tempo.max(20.0);
        let steps = ((secs / 0.25).round() as u32).clamp(4, 400);
        let scene = match &a.scene {
            Some(r) => Some(self.resolve(|v| v.scene(r)).await?.1),
            None => None,
        };
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        self.call(Command::SetTransport {
            change: TransportChange {
                position: Some(a.start_beat.unwrap_or(0.0)),
                automation_write: Some(true),
                automation_mode: Some("write".into()),
                ..TransportChange::default()
            },
        })
        .await?;
        self.set_param(track, device, a.page.as_ref(), &a.param, a.from)
            .await?;
        if snap["transport"]["recording"] != true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        if let Some(scene) = scene {
            self.call(Command::SceneAction {
                scene,
                action: SceneAction::Launch,
            })
            .await?;
        } else {
            self.call(Command::Transport {
                action: TransportAction::Play,
            })
            .await?;
        }
        let begin = tokio::time::Instant::now();
        let mut last = Json::Null;
        for k in 1..=steps {
            tokio::time::sleep_until(
                begin + Duration::from_secs_f64(secs * f64::from(k) / f64::from(steps)),
            )
            .await;
            let t = shaped(f64::from(k) / f64::from(steps), curve);
            let v = a.from + (a.to - a.from) * t;
            last = self
                .set_param(track, device, a.page.as_ref(), &a.param, v)
                .await?;
        }
        self.call(Command::Transport {
            action: TransportAction::Stop,
        })
        .await?;
        let after = self.call(Command::GetProject).await?;
        if after["transport"]["recording"] == true {
            self.call(Command::Transport {
                action: TransportAction::Record,
            })
            .await?;
        }
        self.call(Command::SetTransport {
            change: TransportChange {
                automation_write: Some(false),
                ..TransportChange::default()
            },
        })
        .await?;
        out(&json!({"bars": a.bars, "steps": steps, "end": last, "curve": curve}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn record_arrangement_launches_scenes_in_order_between_record_and_stop() {
        let (s, log) = rig().await;
        let r = s
            .record_arrangement(Parameters(RecordArrangementArgs {
                scenes: Some(vec![
                    ArrangementScene {
                        scene: Ref::Name("intro".into()),
                        bars: Some(1),
                    },
                    ArrangementScene {
                        scene: Ref::Name("verse".into()),
                        bars: Some(1),
                    },
                ]),
                start_beat: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"recorded_bars\":2"), "{r}");
        let order: Vec<String> = log
            .lock()
            .expect("log")
            .iter()
            .filter(|q| matches!(q["cmd"].as_str(), Some("transport" | "scene_action")))
            .map(|q| {
                q["action"].as_str().unwrap_or_default().to_owned()
                    + &q["scene"].as_u64().map_or(String::new(), |n| n.to_string())
            })
            .collect();
        assert_eq!(order[..4], ["stop", "stop_all_clips", "record", "launch0"]);
        assert_eq!(order[4], "launch1");
        assert_eq!(order.last().map(String::as_str), Some("stop"));
    }

    #[tokio::test]
    async fn record_automation_sweeps_the_parameter() {
        let (s, log) = rig().await;
        let r = s
            .record_automation(Parameters(RecordAutomationArgs {
                track: Ref::Name("synth".into()),
                device: Ref::Number(1),
                page: None,
                param: Ref::Number(1),
                from: 0.1,
                to: 0.9,
                bars: 1,
                start_beat: None,
                scene: None,
                curve: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"steps\""), "{r}");
        let sets = sent(&log, "set_device_param");
        assert!(sets.len() >= 5);
        let last = sets.last().expect("last")["value"].as_f64().expect("v");
        assert!((last - 0.9).abs() < 1e-9);
        assert!(sent(&log, "set_transport")[0]["automation_write"] == true);
    }
}
