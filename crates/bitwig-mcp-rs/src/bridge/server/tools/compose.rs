//! Composing: chords, basslines, arpeggios, melodies, drums, the song plan and whole songs.

use super::super::*;

/// Chords.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteChordsArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key, e.g. "A minor", "F# dorian"; needed for Roman numerals.
    pub key: Option<String>,
    /// Chord symbols ("Am7 Fmaj7 C G") or Roman numerals in the key ("i VI III VII").
    pub progression: String,
    /// Beats each chord lasts (default 4 = one 4/4 bar).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave of the chord roots (default 3; C3 = 60).
    pub octave: Option<i32>,
    /// close (smooth voice leading, default), open, drop2 or root.
    pub voicing: Option<theory::Voicing>,
    /// Rhythm per chord as steps, e.g. "x..x..x." (x hit, X accent, . rest, _ tie); default:
    /// held for the whole chord.
    pub rhythm: Option<String>,
    /// Step length of the rhythm: "1/16" (default), "1/8", "1/4", or beats.
    pub step: Option<String>,
    /// Velocity 1-127 (default 90).
    pub velocity: Option<u8>,
}

/// Bassline.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteBasslineArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key, e.g. "A minor"; needed for Roman numerals.
    pub key: Option<String>,
    /// Chords to follow, as symbols or Roman numerals.
    pub progression: String,
    /// Beats each chord lasts (default 4).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave (default 1; C1 = 36).
    pub octave: Option<i32>,
    /// root (default), octave, fifth, arpeggio or walking.
    pub style: Option<theory::BassStyle>,
    /// Hits per chord as steps (default "x.xx" in eighths).
    pub rhythm: Option<String>,
    /// Step length: "1/8" (default), "1/16", ...
    pub step: Option<String>,
    /// Velocity (default 100).
    pub velocity: Option<u8>,
}

/// Arpeggio.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteArpArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key; needed for Roman numerals.
    pub key: Option<String>,
    /// Chords to arpeggiate.
    pub progression: String,
    /// Beats each chord lasts (default 4).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave (default 3).
    pub octave: Option<i32>,
    /// up (default), down, up_down or random.
    pub direction: Option<theory::ArpMode>,
    /// Note length: "1/16" (default), "1/8", "1/8T"...
    pub rate: Option<String>,
    /// Octaves spanned (default 1, max 4).
    pub octaves: Option<u8>,
    /// Gate 0.05-1 of the rate (default 0.8).
    pub gate: Option<f64>,
    /// Velocity (default 90).
    pub velocity: Option<u8>,
    /// Seed for random (default 1).
    pub seed: Option<u64>,
}

/// Melody.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteMelodyArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Key, e.g. "A minor"; the melody stays in it.
    pub key: String,
    /// The chords underneath: symbols or Roman numerals in the key. Notes on a chord change
    /// are chord tones.
    pub progression: String,
    /// Beats each chord lasts (default 4).
    pub beats_per_chord: Option<f64>,
    /// Bitwig octave of the lowest note (default 4).
    pub octave: Option<i32>,
    /// Octaves the melody spans, 1-3 (default 1).
    pub span: Option<u8>,
    /// arch (default), rise, fall, wave or flat: the shape of the motif.
    pub contour: Option<theory::Contour>,
    /// How busy it is, 0-1 (default 0.6).
    pub density: Option<f64>,
    /// Smallest step: "1/8" (default), "1/16", "1/8T"...
    pub rate: Option<String>,
    /// Length of the motif in beats (default 4); it repeats through the clip.
    pub motif_beats: Option<f64>,
    /// Rhythm of the motif as steps ("x.x.xx.." ; x hit, . rest), instead of density.
    pub rhythm: Option<String>,
    /// How much each repetition changes, 0-1 (default 0.3; 0 = exact repeats).
    pub variation: Option<f64>,
    /// Velocity (default 95).
    pub velocity: Option<u8>,
    /// Seed (default 1); another seed is another hook.
    pub seed: Option<u64>,
}

/// One Euclidean voice.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EuclidArg {
    /// Hits.
    pub hits: usize,
    /// Steps.
    pub steps: usize,
    /// Rotation (default 0).
    pub rotate: Option<usize>,
}

/// Drums.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteDrumsArgs {
    /// Track, slot, clip name, mode.
    #[serde(flatten)]
    pub target: Target,
    /// Instrument profile whose named keys to use (default "generic": General MIDI drums,
    /// which Bitwig's Drum Machine follows: kick, snare, clap, closed_hat, open_hat, ...).
    pub instrument: Option<String>,
    /// Voice → steps, e.g. {"kick": "x...x...x...x...", "closed_hat": "..x...x...x...x."}.
    /// Voices are key names from the profile, note names ("C1") or numbers.
    #[serde(default)]
    pub patterns: std::collections::BTreeMap<String, String>,
    /// Voice → Euclidean rhythm, e.g. {"rim": {"hits": 3, "steps": 8}}.
    #[serde(default)]
    pub euclid: std::collections::BTreeMap<String, EuclidArg>,
    /// Step length: "1/16" (default), "1/8"...
    pub step: Option<String>,
    /// Clip length in beats (default 4).
    pub length_beats: Option<f64>,
    /// Velocity (default 100; X accents get +25).
    pub velocity: Option<u8>,
}

/// The song plan.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SongPlanArgs {
    /// The plan as JSON: anything useful, e.g. {"title", "genre", "key", "tempo",
    /// "sections": [{"name", "bars", "scene", "energy"}], "tracks": {"Bass": "role"},
    /// "notes": [...]}. Replaces the stored plan.
    pub plan: Json,
}

/// Build the parts of a song.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct BuildSongArgs {
    /// Sections to build: [{"name": "Verse", "scene": "Verse", "parts": [{"kind": "chords",
    /// "track": "Pad", "progression": "i VI III VII"}, ...]}]. A part is a write_drums,
    /// write_bassline, write_chords, write_arp, write_melody, create_clip or copy_clip call:
    /// `kind` is drums, bass, chords, arp, melody, clip or copy, the other fields are that tool's arguments. `slot`
    /// defaults to the section's `scene` (else its `name`), `key` to the song plan's key.
    /// Default: the `sections` of the stored song plan.
    pub sections: Option<Vec<Json>>,
    /// Build only the sections with these names.
    pub only: Option<Vec<String>>,
}

#[tool_router(router = compose_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Write a chord progression into a clip: chord symbols (\"Am7 Fmaj7 C G\") or Roman numerals in a key (key \"A minor\", \"i VI III VII\"), with voicing (close voice-leads smoothly), octave and an optional rhythm per chord (\"x..x..x.\"). An empty slot gets a new clip."
    )]
    pub(crate) async fn write_chords(
        &self,
        Parameters(a): Parameters<WriteChordsArgs>,
    ) -> ToolResult {
        let chords = chords_of(a.key.as_deref(), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rhythm = rhythm_of(a.rhythm.as_deref())?;
        let step = parse_step(a.step.as_deref(), 0.25)?;
        let notes = theory::chord_notes(
            &chords,
            per,
            a.octave.unwrap_or(3).clamp(-2, 8),
            a.voicing.unwrap_or_default(),
            rhythm.as_deref().map(|r| (r, step)),
            a.velocity.unwrap_or(90),
        );
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Write a bassline that follows a chord progression (symbols or Roman numerals in a key): style root, octave, fifth, arpeggio or walking; rhythm as steps (default \"x.xx\" in eighths); octave 1 by default."
    )]
    pub(crate) async fn write_bassline(
        &self,
        Parameters(a): Parameters<WriteBasslineArgs>,
    ) -> ToolResult {
        let chords = chords_of(a.key.as_deref(), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rhythm = rhythm_of(a.rhythm.as_deref())?;
        let step = parse_step(a.step.as_deref(), 0.5)?;
        let notes = theory::bassline(
            &chords,
            per,
            a.octave.unwrap_or(1).clamp(-2, 8),
            a.style.unwrap_or_default(),
            rhythm.as_deref().map(|r| (r, step)),
            a.velocity.unwrap_or(100),
        );
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Write an arpeggio over a chord progression: direction up, down, up_down or random (seeded), rate (\"1/16\", \"1/8T\"), octaves and gate."
    )]
    pub(crate) async fn write_arp(&self, Parameters(a): Parameters<WriteArpArgs>) -> ToolResult {
        let chords = chords_of(a.key.as_deref(), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rate = parse_step(a.rate.as_deref(), 0.25)?;
        let notes = theory::arpeggio(
            &chords,
            per,
            a.octave.unwrap_or(3).clamp(-2, 8),
            a.direction.unwrap_or_default(),
            rate,
            a.octaves.unwrap_or(1).clamp(1, 4),
            a.gate.unwrap_or(0.8),
            a.velocity.unwrap_or(90),
            a.seed.unwrap_or(1),
        );
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Write a melodic hook in a key over a chord progression: a short motif (rhythm + contour: arch, rise, fall, wave, flat) repeated through the clip with small variations, mostly stepwise, landing on chord tones at chord changes and at the end. Tune density, rate, span, variation; the seed gives other hooks. Run it again with another seed or transform_clip for alternatives."
    )]
    pub(crate) async fn write_melody(
        &self,
        Parameters(a): Parameters<WriteMelodyArgs>,
    ) -> ToolResult {
        let scale = theory::Scale::parse(&a.key).map_err(bad)?;
        let chords = chords_of(Some(&a.key), &a.progression)?;
        let per = beats_of(a.beats_per_chord)?;
        let rate = parse_step(a.rate.as_deref(), 0.5)?;
        let motif = a.motif_beats.unwrap_or(4.0);
        if !(motif.is_finite() && (0.5..=32.0).contains(&motif)) {
            return Err(bad("motif_beats must be 0.5-32"));
        }
        let rhythm = a
            .rhythm
            .as_deref()
            .map(theory::steps)
            .transpose()
            .map_err(bad)?;
        let spec = theory::MelodySpec {
            beats_per_chord: per,
            octave: a.octave.unwrap_or(4).clamp(-2, 8),
            span: a.span.unwrap_or(1).clamp(1, 3),
            contour: a.contour.unwrap_or_default(),
            density: a.density.unwrap_or(0.6).clamp(0.05, 1.0),
            rate,
            motif_beats: motif,
            rhythm,
            variation: a.variation.unwrap_or(0.3).clamp(0.0, 1.0),
            velocity: a.velocity.unwrap_or(95),
            seed: a.seed.unwrap_or(1),
        };
        let notes = theory::melody(&scale, &chords, &spec);
        self.put_notes(&a.target, notes, per * chords.len() as f64)
            .await
    }

    #[tool(
        description = "Build a song from its plan in one call: for every section, run its parts (drums, bass, chords, arp, melody, clip, copy) into the section's scene. Uses the sections of the stored song plan unless given. Each part is a write_* / create_clip / copy_clip call; a failing part is reported and the rest still run. Create the tracks and scenes first."
    )]
    pub(crate) async fn build_song(&self, Parameters(a): Parameters<BuildSongArgs>) -> ToolResult {
        let plan: Json = match &a.sections {
            Some(_) => Json::Null,
            None => {
                let r = self.call(Command::GetSongPlan).await?;
                serde_json::from_str(r["text"].as_str().unwrap_or_default()).unwrap_or(Json::Null)
            }
        };
        let sections = a
            .sections
            .clone()
            .or_else(|| plan["sections"].as_array().cloned())
            .ok_or_else(|| bad("no sections: pass them, or store a plan with sections and parts (set_song_plan)"))?;
        let key = plan["key"].as_str().map(str::to_owned);
        let mut results = Vec::new();
        for sec in &sections {
            let name = sec["name"].as_str().unwrap_or_default();
            if let Some(only) = &a.only
                && !only.iter().any(|o| o.eq_ignore_ascii_case(name))
            {
                continue;
            }
            let scene = sec["scene"].as_str().unwrap_or(name);
            for part in sec["parts"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let mut part = part.clone();
                let kind = part["kind"].as_str().unwrap_or_default().to_owned();
                if let Some(o) = part.as_object_mut() {
                    o.remove("kind");
                    if kind != "copy" && !scene.is_empty() {
                        o.entry("slot").or_insert_with(|| json!(scene));
                    }
                    if let (Some(k), true) = (
                        &key,
                        matches!(kind.as_str(), "bass" | "chords" | "arp" | "melody"),
                    ) {
                        o.entry("key").or_insert_with(|| json!(k));
                    }
                }
                let track = part["track"].clone();
                let r = self.build_part(&kind, part).await;
                results.push(match r {
                    Ok(_) => json!({"section": name, "kind": kind, "track": track, "ok": true}),
                    Err(e) => {
                        json!({"section": name, "kind": kind, "track": track, "error": e.message})
                    }
                });
            }
        }
        if results.is_empty() {
            return Err(bad(
                "no parts to build (check `only` and the sections' parts)",
            ));
        }
        let failed = results.iter().filter(|r| r.get("error").is_some()).count();
        out(&json!({"built": results.len() - failed, "failed": failed, "parts": results}))
    }

    #[tool(
        description = "Write a drum pattern: per voice a step string (\"x...x...x...x...\", X = accent, _ = tie) and/or a Euclidean rhythm. Voices are named keys of an instrument profile (default General MIDI, which Bitwig's Drum Machine uses: kick, snare, clap, closed_hat, open_hat, low_tom, mid_tom, high_tom, crash, ride), note names or numbers."
    )]
    pub(crate) async fn write_drums(
        &self,
        Parameters(a): Parameters<WriteDrumsArgs>,
    ) -> ToolResult {
        if a.patterns.is_empty() && a.euclid.is_empty() {
            return Err(bad("give patterns and/or euclid"));
        }
        let registry = self.profiles();
        let pname = a.instrument.as_deref().unwrap_or("generic");
        let profile = registry
            .get(pname)
            .ok_or_else(|| bad(format!("no instrument profile {pname:?}")))?;
        let step = parse_step(a.step.as_deref(), 0.25)?;
        let length = a.length_beats.unwrap_or(4.0);
        if !(length.is_finite() && (0.25..=256.0).contains(&length)) {
            return Err(bad("length_beats must be 0.25-256"));
        }
        let mut voices = Vec::new();
        for (voice, pattern) in &a.patterns {
            voices.push((
                voice_key(Some(profile), voice)?,
                theory::steps(pattern).map_err(bad)?,
            ));
        }
        for (voice, e) in &a.euclid {
            if e.steps == 0 || e.steps > 64 {
                return Err(bad("euclid steps must be 1-64"));
            }
            voices.push((
                voice_key(Some(profile), voice)?,
                theory::euclid(e.hits, e.steps, e.rotate.unwrap_or(0)),
            ));
        }
        let notes = theory::drums(&voices, step, length, a.velocity.unwrap_or(100));
        self.put_notes(&a.target, notes, length).await
    }

    #[tool(
        description = "The song plan stored in this Bitwig project: key, tempo, sections, track roles and ideas from earlier sessions. Read it before continuing a song."
    )]
    pub(crate) async fn get_song_plan(&self) -> ToolResult {
        let r = self.call(Command::GetSongPlan).await?;
        let text = r["text"].as_str().unwrap_or_default();
        if text.trim().is_empty() {
            return out(
                &json!({"plan": null, "hint": "no plan yet: set_song_plan after deciding key, tempo and sections"}),
            );
        }
        let plan: Json = serde_json::from_str(text).unwrap_or(Json::String(text.to_owned()));
        out(&json!({"plan": plan}))
    }

    #[tool(
        description = "Store the song plan (JSON) in the Bitwig project, so later sessions can continue the song: title, genre, key, tempo, sections with bars and scenes, track roles, decisions. Replaces the stored plan; saved with the project."
    )]
    pub(crate) async fn set_song_plan(
        &self,
        Parameters(a): Parameters<SongPlanArgs>,
    ) -> ToolResult {
        let text = serde_json::to_string(&a.plan).map_err(err)?;
        if text.chars().count() > 60_000 {
            return Err(bad("the plan is limited to 60000 characters"));
        }
        out(&self.call(Command::SetSongPlan { text }).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn write_chords_creates_a_clip_and_writes_voiced_chords() {
        let (s, log) = rig().await;
        let r = s
            .write_chords(Parameters(WriteChordsArgs {
                target: Target {
                    track: Ref::Name("synth".into()),
                    slot: Ref::Name("verse".into()),
                    name: Some("chords".into()),
                    mode: None,
                },
                key: Some("A minor".into()),
                progression: "i VI III VII".into(),
                beats_per_chord: None,
                octave: None,
                voicing: None,
                rhythm: None,
                step: None,
                velocity: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"new_clip\":true"), "{r}");
        assert_eq!(sent(&log, "create_clip")[0]["length"], 16);
        let w = &sent(&log, "write_notes")[0];
        assert_eq!(w["notes"].as_array().map(Vec::len), Some(12), "4 triads");
        assert_eq!(w["step"], 1.0, "chords on the coarsest (beat) grid");
        assert_eq!(sent(&log, "set_clip")[0]["name"], "chords");
    }

    #[tokio::test]
    async fn write_drums_maps_voices_to_general_midi_keys() {
        let (s, log) = rig().await;
        let mut patterns = std::collections::BTreeMap::new();
        patterns.insert("kick".to_owned(), "x...x...x...x...".to_owned());
        let mut euclid = std::collections::BTreeMap::new();
        euclid.insert(
            "closed_hat".to_owned(),
            EuclidArg {
                hits: 5,
                steps: 8,
                rotate: None,
            },
        );
        s.write_drums(Parameters(WriteDrumsArgs {
            target: Target {
                track: Ref::Number(1),
                slot: Ref::Number(1),
                name: None,
                mode: None,
            },
            instrument: None,
            patterns,
            euclid,
            step: None,
            length_beats: None,
            velocity: None,
        }))
        .await
        .expect("tool");
        let notes = sent(&log, "write_notes")[0]["notes"].clone();
        let keys: Vec<u64> = notes
            .as_array()
            .expect("notes")
            .iter()
            .filter_map(|n| n["key"].as_u64())
            .collect();
        assert_eq!(keys.iter().filter(|k| **k == 36).count(), 4);
        assert_eq!(keys.iter().filter(|k| **k == 42).count(), 10);
    }

    #[tokio::test]
    async fn composing_rejects_bad_input_before_sending() {
        let (s, log) = rig().await;
        let target = || Target {
            track: Ref::Number(1),
            slot: Ref::Number(1),
            name: None,
            mode: None,
        };
        let e = s
            .write_bassline(Parameters(WriteBasslineArgs {
                target: target(),
                key: None,
                progression: "i iv v".into(),
                beats_per_chord: None,
                octave: None,
                style: None,
                rhythm: None,
                step: None,
                velocity: None,
            }))
            .await
            .expect_err("roman without key");
        assert!(e.message.contains("key"));
        assert!(
            s.write_arp(Parameters(WriteArpArgs {
                target: target(),
                key: None,
                progression: "Am Hm".into(),
                beats_per_chord: None,
                octave: None,
                direction: None,
                rate: None,
                octaves: None,
                gate: None,
                velocity: None,
                seed: None,
            }))
            .await
            .is_err()
        );
        assert!(sent(&log, "create_clip").is_empty() && sent(&log, "write_notes").is_empty());
    }

    #[tokio::test]
    async fn song_plan_round_trips_as_json() {
        let (s, log) = rig().await;
        s.set_song_plan(Parameters(SongPlanArgs {
            plan: json!({"key": "A minor", "tempo": 124}),
        }))
        .await
        .expect("set");
        let stored = sent(&log, "set_song_plan")[0]["text"]
            .as_str()
            .expect("text")
            .to_owned();
        assert_eq!(
            serde_json::from_str::<Json>(&stored).expect("json")["tempo"],
            124
        );
        let got = s.get_song_plan().await.expect("get");
        assert!(got.contains("A minor"), "{got}");
    }

    #[tokio::test]
    async fn build_song_runs_parts_into_the_section_scenes() {
        let (s, log) = rig().await;
        let r = s
            .build_song(Parameters(BuildSongArgs {
                sections: Some(vec![json!({
                    "name": "Verse",
                    "parts": [
                        {"kind": "chords", "track": "Synth", "progression": "i VI", "key": "A minor"},
                        {"kind": "bass", "track": "Synth", "progression": "i VI", "key": "A minor"},
                        {"kind": "ufo", "track": "Synth"}
                    ]
                })]),
                only: None,
            }))
            .await
            .expect("tool");
        assert!(
            r.contains("\"built\":2") && r.contains("\"failed\":1"),
            "{r}"
        );
        assert_eq!(sent(&log, "write_notes").len(), 2);
        assert!(r.contains("ufo"), "{r}");
    }

    #[tokio::test]
    async fn write_melody_writes_notes_in_the_key() {
        let (s, log) = rig().await;
        let r = s
            .write_melody(Parameters(WriteMelodyArgs {
                target: Target {
                    track: Ref::Name("synth".into()),
                    slot: Ref::Name("verse".into()),
                    name: Some("hook".into()),
                    mode: None,
                },
                key: "A minor".into(),
                progression: "i VI III VII".into(),
                beats_per_chord: None,
                octave: None,
                span: None,
                contour: None,
                density: None,
                rate: None,
                motif_beats: None,
                rhythm: None,
                variation: None,
                velocity: None,
                seed: None,
            }))
            .await
            .expect("tool");
        assert!(r.contains("\"new_clip\":true"), "{r}");
        assert_eq!(sent(&log, "create_clip")[0]["length"], 16);
        let scale = theory::Scale::parse("A minor").expect("key");
        let w = &sent(&log, "write_notes")[0];
        let notes = w["notes"].as_array().expect("notes");
        assert!(notes.len() >= 8);
        assert!(
            notes
                .iter()
                .all(|n| scale.contains(n["key"].as_u64().expect("key") as u8))
        );
    }
}
