//! Export: MIDI files and audio capture.

use super::super::*;

/// A clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipRef {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
}

/// MIDI export.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportMidiArgs {
    /// Export every clip of this scene, one MIDI track per Bitwig track.
    pub scene: Option<Ref>,
    /// Or these clips.
    #[serde(default)]
    pub clips: Vec<ClipRef>,
    /// File name (without folder) or absolute path; default: PROJECT-TIME.mid in
    /// the music folder's bitwig-mcp-rs/.
    pub path: Option<String>,
}

/// Audio capture.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CaptureArgs {
    /// Bars to record at the project tempo (default 8; max 512).
    pub bars: Option<u32>,
    /// Launch this scene from bar 1 (launcher songs); default: play the arranger from the
    /// start.
    pub scene: Option<Ref>,
    /// wav (default, 32-bit float), flac or mp3.
    pub format: Option<capture::Format>,
    /// Also record each track alone (stems), one pass per track: takes bars x tracks of time.
    pub stems: Option<bool>,
    /// Seconds recorded after the last bar for reverb and delay tails (default 2).
    pub tail_seconds: Option<f64>,
    /// File name (without folder) or absolute path, without extension.
    pub path: Option<String>,
}

#[tool_router(router = export_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Export clips as a standard MIDI file (.mid): every clip of a scene (one MIDI track per Bitwig track) or a list of clips. Writes into the music folder's bitwig-mcp-rs/ unless a path is given."
    )]
    pub(crate) async fn export_midi(
        &self,
        Parameters(a): Parameters<ExportMidiArgs>,
    ) -> ToolResult {
        let (snap, view) = self.project().await?;
        let mut picks: Vec<(TrackAddr, u32)> = Vec::new();
        if let Some(scene) = &a.scene {
            let slot = view.slot(scene).map_err(bad)?;
            for t in view.tracks.iter().chain(&view.effect_tracks) {
                if t.clips.iter().any(|c| c.slot == slot) {
                    picks.push((t.addr(), slot));
                }
            }
        }
        for c in &a.clips {
            let track = view.track(&c.track).map_err(bad)?;
            let slot = view.slot(&c.slot).map_err(bad)?;
            if !view.has_clip(track, slot) {
                return Err(bad(format!("slot {} of {} has no clip", slot + 1, c.track)));
            }
            picks.push((track, slot));
        }
        if picks.is_empty() {
            return Err(bad("nothing to export: give a scene with clips or clips"));
        }
        let mut tracks = Vec::new();
        for (track, slot) in &picks {
            let (notes, _) = self.read_clip_notes(*track, *slot).await?;
            let name = view
                .track_view(*track)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            tracks.push(crate::smf::SmfTrack {
                name,
                channel: 1,
                notes,
            });
        }
        let (tempo, _, num, den) = tempo_of(&snap);
        let project = snap["application"]["project_name"]
            .as_str()
            .unwrap_or("bitwig");
        let stem = format!("{project}-{}", capture::stamp());
        let path = export_path(a.path.as_deref(), &stem, "mid");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let bytes = crate::smf::write(&tracks, tempo, num, den);
        std::fs::write(&path, &bytes).map_err(|e| err(format!("{}: {e}", path.display())))?;
        out(&json!({
            "path": path.display().to_string(),
            "tracks": tracks.iter().map(|t| json!({"name": t.name, "notes": t.notes.len()})).collect::<Vec<_>>(),
            "tempo": tempo,
        }))
    }

    #[tool(
        description = "Export audio by recording Bitwig's output in real time (Linux, PipeWire): plays the arranger from the start, or a scene from bar 1, for N bars and writes WAV, FLAC or MP3 into the music folder's bitwig-mcp-rs/. Optional stems (each track alone, one pass each). Reports loudness (LUFS, true peak) with a short verdict, so you can check the mix."
    )]
    pub(crate) async fn capture_audio(&self, Parameters(a): Parameters<CaptureArgs>) -> ToolResult {
        let bars = a.bars.unwrap_or(8);
        if !(1..=512).contains(&bars) {
            return Err(bad("bars must be 1-512"));
        }
        let tail = a.tail_seconds.unwrap_or(2.0).clamp(0.0, 30.0);
        let format = a.format.unwrap_or_default();
        let (snap, view) = self.project().await?;
        let scene = a
            .scene
            .as_ref()
            .map(|s| view.scene(s))
            .transpose()
            .map_err(bad)?;
        let (tempo, beats_per_bar, _, _) = tempo_of(&snap);
        let seconds = f64::from(bars) * beats_per_bar * 60.0 / tempo;
        let project = snap["application"]["project_name"]
            .as_str()
            .unwrap_or("bitwig")
            .to_owned();
        let base = export_path(
            a.path.as_deref(),
            &format!("{project}-{}", capture::stamp()),
            format.ext(),
        );

        // passes: the mix, then one per track with clips when stems are asked for
        let mut passes: Vec<(String, Option<TrackAddr>)> = vec![("mix".into(), None)];
        if a.stems.unwrap_or(false) {
            for t in &view.tracks {
                if !t.clips.is_empty() || !t.devices.is_empty() {
                    passes.push((t.name.clone(), Some(t.addr())));
                }
            }
        }
        let mut files = Vec::new();
        for (label, solo) in passes {
            let file = if solo.is_some() {
                let stem = base
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                base.with_file_name(format!("{stem}-{}", capture::safe_name(&label)))
                    .with_extension(format.ext())
            } else {
                base.clone()
            };
            if let Some(t) = solo {
                self.call(Command::SetTrack {
                    track: t,
                    change: TrackChange {
                        solo: Some(true),
                        ..TrackChange::default()
                    },
                })
                .await?;
            }
            let wav = file.with_extension("capture.wav");
            let bridge = Arc::clone(&self.bridge);
            let result = capture::record(
                capture::BITWIG_NODE,
                &wav,
                Duration::from_secs_f64(tail),
                || async move {
                    bridge
                        .call(Command::Transport {
                            action: TransportAction::Stop,
                        })
                        .await?;
                    bridge
                        .call(Command::SetTransport {
                            change: TransportChange {
                                position: Some(0.0),
                                ..TransportChange::default()
                            },
                        })
                        .await?;
                    if let Some(sc) = scene {
                        bridge
                            .call(Command::SceneAction {
                                scene: sc,
                                action: SceneAction::Launch,
                            })
                            .await?;
                    }
                    bridge
                        .call(Command::Transport {
                            action: TransportAction::Play,
                        })
                        .await?;
                    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                    bridge
                        .call(Command::Transport {
                            action: TransportAction::Stop,
                        })
                        .await?;
                    Ok(())
                },
            )
            .await;
            if let Some(t) = solo {
                self.call(Command::SetTrack {
                    track: t,
                    change: TrackChange {
                        solo: Some(false),
                        ..TrackChange::default()
                    },
                })
                .await?;
            }
            result.map_err(err)?;
            // the stream starts 0.4 s before the music: trim it
            capture::encode(&wav, &file, format, 0.4)
                .await
                .map_err(err)?;
            let loudness = capture::measure(&file).await.unwrap_or_default();
            files.push(json!({
                "part": label,
                "path": file.display().to_string(),
                "loudness": loudness,
                "verdict": loudness.verdict(),
            }));
        }
        out(&json!({"bars": bars, "seconds": seconds + tail, "files": files}))
    }
}
