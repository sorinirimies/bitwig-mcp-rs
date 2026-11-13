//! Clips and their notes: writing, reading, copying, transforming, recording.

use super::super::*;

/// One note.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NoteArg {
    /// 60 or "C3".
    pub note: NoteRef,
    /// Start in beats from the clip start.
    pub start: f64,
    /// Length in beats.
    pub length: f64,
    /// Velocity 1-127 (default 100).
    pub velocity: Option<u8>,
}

/// A new clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateClipArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Length in whole beats (4 = one bar of 4/4).
    pub length_beats: u32,
    /// Clip name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// Notes to write (exact positions, no live recording).
    #[serde(default)]
    pub notes: Vec<NoteArg>,
}

/// Notes for an existing clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteNotesArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// The notes.
    pub notes: Vec<NoteArg>,
    /// Replace the clip's notes (default) or add to them.
    pub replace: Option<bool>,
}

/// A clip to read.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadNotesArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Grid as a note value: "1/16" (default, a sixteenth note), "1/32", "1/8T"; starts are
    /// reported on this grid.
    pub resolution: Option<String>,
}

/// Clip action.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipActionArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// launch, stop, select, record, delete, duplicate or show.
    pub action: ClipAction,
}

/// Clip settings.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetClipArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// New name.
    pub name: Option<String>,
    /// "#rrggbb" or a color name.
    pub color: Option<String>,
    /// Loop length in beats.
    pub loop_length_beats: Option<f64>,
    /// Loop on/off.
    pub r#loop: Option<bool>,
    /// Transpose all notes by semitones.
    pub transpose: Option<i32>,
    /// Quantize note starts, amount 0.0-1.0.
    pub quantize: Option<f64>,
}

/// Replace a clip's notes or add to them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Clear the clip first.
    #[default]
    Replace,
    /// Keep existing notes.
    Add,
}

/// Where generated notes go.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Target {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name. An empty slot gets a new clip.
    pub slot: Ref,
    /// Clip name.
    pub name: Option<String>,
    /// replace (default) or add.
    pub mode: Option<WriteMode>,
}

/// Transform a clip.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransformClipArgs {
    /// Track name or 1-based number.
    pub track: Ref,
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Operations in order, e.g. [{"op": "transpose", "semitones": 7},
    /// {"op": "humanize", "timing": 0.02, "velocity": 8}].
    pub ops: Vec<theory::Transform>,
}

/// One place a clip is copied to.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CopyDest {
    /// Slot: 1-based number or scene name.
    pub slot: Ref,
    /// Variation for this copy only, e.g. [{"op": "transpose", "semitones": 7}] or
    /// [{"op": "thin", ...}]; same operations as transform_clip.
    pub ops: Option<Vec<theory::Transform>>,
    /// Name of the copy; default: unchanged.
    pub name: Option<String>,
}

/// Copy a clip into other slots.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CopyClipArgs {
    /// Source track name or 1-based number.
    pub track: Ref,
    /// Source slot: 1-based number or scene name.
    pub slot: Ref,
    /// Target track; default: the source track.
    pub to_track: Option<Ref>,
    /// The slots to copy to, each with an optional variation. Existing clips there are replaced.
    pub to: Vec<CopyDest>,
}

/// Record into a slot.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordClipArgs {
    /// Track to record on: an audio track whose input is the synth (or a mic), or an
    /// instrument track for MIDI.
    pub track: Ref,
    /// Slot: 1-based number or scene name (should be empty).
    pub slot: Ref,
    /// Length in bars (1-64).
    pub bars: u32,
    /// After recording: play (default) or stop.
    pub then: Option<String>,
    /// Notes to play into it while recording (e.g. to capture a hardware synth): same format
    /// as create_clip. Needs a HW Instrument on `play_on`.
    #[serde(default)]
    pub notes: Vec<NoteArg>,
    /// Track that plays `notes` (e.g. the HW Instrument track of the synth being recorded).
    pub play_on: Option<Ref>,
}

/// A note given by number or name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum NoteRef {
    /// MIDI note 0-127 (60 = C3).
    Number(u8),
    /// Name like "C3", "F#2", "Bb1".
    Name(String),
}

impl NoteRef {
    pub(crate) fn key(&self) -> Result<u8, McpError> {
        match self {
            Self::Number(n) if *n <= 127 => Ok(*n),
            Self::Number(n) => Err(bad(format!("note {n} is above 127"))),
            Self::Name(s) => music::parse_note(s).map_err(bad),
        }
    }
}

#[tool_router(router = clips_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Create a launcher clip in a slot (slot = 1-based number or scene name) with exact notes: [{note: \"C3\" or 60, start: beats, length: beats, velocity}]. Notes are written directly, not recorded. Replies with the notes as Bitwig stored them."
    )]
    pub(crate) async fn create_clip(
        &self,
        Parameters(a): Parameters<CreateClipArgs>,
    ) -> ToolResult {
        if a.length_beats == 0 || a.length_beats > 1024 {
            return Err(bad("length_beats must be 1-1024"));
        }
        if let Some(late) = a
            .notes
            .iter()
            .find(|n| n.start >= f64::from(a.length_beats))
        {
            return Err(bad(format!(
                "a note starts at beat {} but the clip is {} beats long",
                late.start, a.length_beats
            )));
        }
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let color = a
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(bad)?;
        let grid = Self::grid_notes(&a.notes)?;
        self.call(Command::CreateClip {
            track,
            slot,
            length: a.length_beats,
        })
        .await?;
        if a.name.is_some() || color.is_some() {
            self.call(Command::SetClip {
                track,
                slot,
                change: ClipChange {
                    name: a.name,
                    color,
                    ..ClipChange::default()
                },
            })
            .await?;
        }
        let written = if grid.1.is_empty() {
            json!({"notes": []})
        } else {
            self.call(Command::WriteNotes {
                track,
                slot,
                step: grid.0,
                clear: true,
                notes: grid.1,
            })
            .await?
        };
        out(&json!({"slot": slot + 1, "written": written, "rounded_to_1_64": grid.2}))
    }

    #[tool(
        description = "Write notes into an existing clip at exact positions; replace (default true) clears the old notes first."
    )]
    pub(crate) async fn write_notes(
        &self,
        Parameters(a): Parameters<WriteNotesArgs>,
    ) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let (step, notes, rounded) = Self::grid_notes(&a.notes)?;
        let r = self
            .call(Command::WriteNotes {
                track,
                slot,
                step,
                clear: a.replace.unwrap_or(true),
                notes,
            })
            .await?;
        out(&json!({"written": r, "rounded_to_1_64": rounded}))
    }

    #[tool(
        description = "Read the notes of a clip: [{note, name, start, length, velocity}] in beats, starts on the given grid (default 1/16)."
    )]
    pub(crate) async fn read_notes(&self, Parameters(a): Parameters<ReadNotesArgs>) -> ToolResult {
        let step = parse_step(a.resolution.as_deref(), 0.25)?;
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let r = self.call(Command::ReadNotes { track, slot, step }).await?;
        let notes: Vec<Json> = r["notes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| {
                        let key = n["key"].as_u64().unwrap_or(0).min(127) as u8;
                        let x = n["x"].as_f64().unwrap_or(0.0);
                        json!({
                            "note": key,
                            "name": music::note_name(key),
                            "start": x * step,
                            "length": n["duration"],
                            "velocity": n["velocity"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out(&json!({"clip": r["clip"], "resolution_beats": step, "notes": notes}))
    }

    #[tool(
        description = "Clip action on a slot: launch, stop, select, record, delete, duplicate, show."
    )]
    pub(crate) async fn clip_action(
        &self,
        Parameters(a): Parameters<ClipActionArgs>,
    ) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        out(&self
            .call(Command::ClipAction {
                track,
                slot,
                action: a.action,
            })
            .await?)
    }

    #[tool(
        description = "Change a clip: name, color, loop length, loop on/off, transpose (semitones), quantize (0-1)."
    )]
    pub(crate) async fn set_clip(&self, Parameters(a): Parameters<SetClipArgs>) -> ToolResult {
        let (view, track) = self.track_of(&a.track).await?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        let change = ClipChange {
            name: a.name,
            color: a
                .color
                .as_deref()
                .map(parse_color)
                .transpose()
                .map_err(bad)?,
            loop_length: a.loop_length_beats,
            loop_enabled: a.r#loop,
            transpose: a.transpose,
            quantize: unit("quantize", a.quantize)?,
        };
        out(&self
            .call(Command::SetClip {
                track,
                slot,
                change,
            })
            .await?)
    }

    #[tool(
        description = "Transform the notes of a clip in place: transpose, to_scale (snap to a key), humanize (seeded timing/velocity), quantize, reverse, stretch (0.5 = double time, 2 = half time), velocity (scale/offset), thin. Ops run in order."
    )]
    pub(crate) async fn transform_clip(
        &self,
        Parameters(a): Parameters<TransformClipArgs>,
    ) -> ToolResult {
        if a.ops.is_empty() {
            return Err(bad("ops is empty"));
        }
        let (_, (track, slot)) = self
            .resolve(|v| {
                let track = v.track(&a.track)?;
                let slot = v.slot(&a.slot)?;
                if v.has_clip(track, slot) {
                    Ok((track, slot))
                } else {
                    Err(format!("slot {} has no clip", slot + 1))
                }
            })
            .await?;
        let (notes, length) = self.read_clip_notes(track, slot).await?;
        if notes.is_empty() {
            return Err(bad("the clip has no notes"));
        }
        let changed = theory::transform(&notes, &a.ops, length).map_err(bad)?;
        let target = Target {
            track: a.track,
            slot: Ref::Number(slot + 1),
            name: None,
            mode: Some(WriteMode::Replace),
        };
        let r = self.put_notes(&target, changed, length).await?;
        out(
            &json!({"before": notes.len(), "result": serde_json::from_str::<Json>(&r).unwrap_or(Json::Null)}),
        )
    }

    #[tool(
        description = "Copy a clip's notes into other slots (scenes), on the same track or another, each copy optionally varied (transpose, thin, humanize, ...). Replaces what is in the target slots. Saves rewriting the same pad for every section."
    )]
    pub(crate) async fn copy_clip(&self, Parameters(a): Parameters<CopyClipArgs>) -> ToolResult {
        if a.to.is_empty() {
            return Err(bad("to is empty"));
        }
        let (_, (track, slot)) = self
            .resolve(|v| {
                let track = v.track(&a.track)?;
                let slot = v.slot(&a.slot)?;
                if v.has_clip(track, slot) {
                    Ok((track, slot))
                } else {
                    Err(format!("slot {} has no clip", slot + 1))
                }
            })
            .await?;
        let (notes, length) = self.read_clip_notes(track, slot).await?;
        if notes.is_empty() {
            return Err(bad("the source clip has no notes"));
        }
        let dest_track = a.to_track.clone().unwrap_or_else(|| a.track.clone());
        let mut done = Vec::new();
        for d in &a.to {
            let copy = match d.ops.as_deref() {
                Some(ops) if !ops.is_empty() => {
                    theory::transform(&notes, ops, length).map_err(bad)?
                }
                _ => notes.clone(),
            };
            if copy.is_empty() {
                return Err(bad(format!("the variation for {} left no notes", d.slot)));
            }
            let target = Target {
                track: dest_track.clone(),
                slot: d.slot.clone(),
                name: d.name.clone(),
                mode: Some(WriteMode::Replace),
            };
            self.put_notes(&target, copy.clone(), length).await?;
            done.push(json!({"slot": d.slot, "notes": copy.len()}));
        }
        out(&json!({"source_notes": notes.len(), "length_beats": length, "copied": done}))
    }

    #[tool(
        description = "Record a launcher clip for a number of bars at the project tempo, then play or stop it. For a hardware synth: record on an audio track whose input is the synth, and pass notes + play_on (its HW Instrument track) to play a part into the recording. The call returns when recording is done."
    )]
    pub(crate) async fn record_clip(
        &self,
        Parameters(a): Parameters<RecordClipArgs>,
    ) -> ToolResult {
        if !(1..=64).contains(&a.bars) {
            return Err(bad("bars must be 1-64"));
        }
        let then_play = match a.then.as_deref() {
            None | Some("play") => true,
            Some("stop") => false,
            Some(o) => return Err(bad(format!("then must be play or stop, not {o:?}"))),
        };
        let (snap, view) = self.project().await?;
        let track = view.track(&a.track).map_err(bad)?;
        let slot = view.slot(&a.slot).map_err(bad)?;
        if view.has_clip(track, slot) {
            return Err(bad(format!("slot {} already holds a clip", slot + 1)));
        }
        let player = a
            .play_on
            .as_ref()
            .map(|r| view.track(r))
            .transpose()
            .map_err(bad)?;
        if !a.notes.is_empty() && player.is_none() {
            return Err(bad("notes need play_on: the track that plays them"));
        }
        let tempo = snap["transport"]["tempo"]
            .as_f64()
            .unwrap_or(120.0)
            .max(20.0);
        let beats_per_bar = snap["transport"]["time_signature"]
            .as_str()
            .and_then(|s| s.split_once('/'))
            .and_then(|(n, d)| {
                Some(n.trim().parse::<f64>().ok()? * 4.0 / d.trim().parse::<f64>().ok()?)
            })
            .unwrap_or(4.0);
        let beats = f64::from(a.bars) * beats_per_bar;
        let seconds = beats * 60.0 / tempo;
        let keys: Vec<(f64, f64, u8, u8)> = a
            .notes
            .iter()
            .map(|n| Ok((n.start, n.length, n.note.key()?, n.velocity.unwrap_or(100))))
            .collect::<Result<_, McpError>>()?;
        self.call(Command::SetTrack {
            track,
            change: TrackChange {
                arm: Some(true),
                ..TrackChange::default()
            },
        })
        .await?;
        self.call(Command::ClipAction {
            track,
            slot,
            action: ClipAction::Record,
        })
        .await?;
        // wait for the launch quantization to start the recording
        let started = std::time::Instant::now();
        loop {
            let p = self.call(Command::GetProject).await?;
            if track_clip(&p, track, slot)["recording"] == true {
                break;
            }
            if started.elapsed() > Duration::from_secs(20) {
                return Err(err(
                    "recording did not start within 20 s (is the transport playing or the launch quantization long?)",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let t0 = tokio::time::Instant::now();
        if let Some(p) = player {
            for (start, len, key, vel) in &keys {
                let at = t0 + Duration::from_secs_f64((start * 60.0 / tempo).max(0.0));
                tokio::time::sleep_until(at).await;
                let ms = (len * 60_000.0 / tempo).clamp(10.0, 10_000.0) as u32;
                let call = Command::PlayNotes {
                    track: p,
                    keys: vec![*key],
                    velocity: *vel,
                    duration_ms: ms,
                };
                let bridge = Arc::clone(&self.bridge);
                tokio::spawn(async move { bridge.call(call).await });
            }
        }
        tokio::time::sleep_until(t0 + Duration::from_secs_f64(seconds)).await;
        let finish = if then_play {
            ClipAction::Launch
        } else {
            ClipAction::Stop
        };
        self.call(Command::ClipAction {
            track,
            slot,
            action: finish,
        })
        .await?;
        self.call(Command::SetTrack {
            track,
            change: TrackChange {
                arm: Some(false),
                ..TrackChange::default()
            },
        })
        .await?;
        out(
            &json!({"recorded_bars": a.bars, "seconds": seconds, "slot": slot + 1, "then": if then_play { "play" } else { "stop" }}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn copy_clip_writes_each_copy_with_its_own_variation() {
        let (s, log) = rig().await;
        let r = s
            .copy_clip(Parameters(CopyClipArgs {
                track: Ref::Name("synth".into()),
                slot: Ref::Name("intro".into()),
                to_track: None,
                to: vec![
                    CopyDest {
                        slot: Ref::Name("verse".into()),
                        ops: None,
                        name: Some("pad 2".into()),
                    },
                    CopyDest {
                        slot: Ref::Number(5),
                        ops: Some(vec![theory::Transform::Transpose { semitones: 12 }]),
                        name: None,
                    },
                ],
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"source_notes\":2"), "{r}");
        let w = sent(&log, "write_notes");
        assert_eq!(w.len(), 2);
        assert_eq!(w[0]["clear"], true);
        let keys = |i: usize| -> Vec<u64> {
            w[i]["notes"]
                .as_array()
                .expect("notes")
                .iter()
                .map(|n| n["key"].as_u64().expect("key"))
                .collect()
        };
        assert_eq!(keys(0), vec![60, 64]);
        assert_eq!(keys(1), vec![72, 76]);
        assert_eq!(sent(&log, "set_clip")[0]["name"], "pad 2");
    }

    #[tokio::test]
    async fn transform_clip_needs_a_clip() {
        let (s, _) = rig().await;
        let e = s
            .transform_clip(Parameters(TransformClipArgs {
                track: Ref::Number(1),
                slot: Ref::Number(1),
                ops: vec![theory::Transform::Transpose { semitones: 12 }],
            }))
            .await
            .expect_err("empty slot");
        assert!(e.message.contains("no clip"));
    }

    #[tokio::test]
    async fn create_clip_writes_notes_on_the_coarsest_grid() {
        let (s, log) = rig().await;
        s.create_clip(Parameters(CreateClipArgs {
            track: Ref::Number(2),
            slot: Ref::Name("verse".into()),
            length_beats: 4,
            name: Some("Riff".into()),
            color: None,
            notes: vec![
                NoteArg {
                    note: NoteRef::Name("C3".into()),
                    start: 0.0,
                    length: 0.5,
                    velocity: None,
                },
                NoteArg {
                    note: NoteRef::Number(67),
                    start: 1.5,
                    length: 0.5,
                    velocity: Some(90),
                },
            ],
        }))
        .await
        .expect("tool");
        assert_eq!(sent(&log, "create_clip")[0]["slot"], 1);
        assert_eq!(sent(&log, "set_clip")[0]["name"], "Riff");
        let w = &sent(&log, "write_notes")[0];
        assert_eq!(w["step"], 0.5);
        assert_eq!(w["clear"], true);
        assert_eq!(
            w["notes"][0],
            json!({"x": 0, "key": 60, "velocity": 100, "duration": 0.5})
        );
        assert_eq!(w["notes"][1]["x"], 3);
    }

    #[tokio::test]
    async fn create_clip_rejects_notes_past_the_end() {
        let (s, log) = rig().await;
        let e = s
            .create_clip(Parameters(CreateClipArgs {
                track: Ref::Number(1),
                slot: Ref::Number(1),
                length_beats: 4,
                name: None,
                color: None,
                notes: vec![NoteArg {
                    note: NoteRef::Number(60),
                    start: 4.0,
                    length: 1.0,
                    velocity: None,
                }],
            }))
            .await
            .expect_err("late note");
        assert!(e.message.contains("4 beats"));
        assert!(sent(&log, "create_clip").is_empty());
    }
}
