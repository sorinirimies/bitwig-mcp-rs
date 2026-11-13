//! Pure helpers: parsing, unit conversion, shaping and summarizing. No bridge, no I/O.

use super::*;

/// Parses a color: `#rrggbb`, `rrggbb`, or a basic name.
///
/// # Errors
/// Unknown formats.
pub(crate) fn parse_color(s: &str) -> Result<Rgb, String> {
    let named = match s.trim().to_lowercase().as_str() {
        "red" => Some(Rgb(217, 46, 36)),
        "orange" => Some(Rgb(255, 131, 62)),
        "yellow" => Some(Rgb(228, 183, 78)),
        "green" => Some(Rgb(115, 152, 20)),
        "teal" | "cyan" => Some(Rgb(0, 166, 148)),
        "blue" => Some(Rgb(0, 153, 217)),
        "purple" | "violet" => Some(Rgb(149, 73, 203)),
        "pink" | "magenta" => Some(Rgb(217, 56, 113)),
        "brown" => Some(Rgb(163, 121, 67)),
        "grey" | "gray" => Some(Rgb(127, 127, 127)),
        "white" => Some(Rgb(255, 255, 255)),
        "black" => Some(Rgb(0, 0, 0)),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(c);
    }
    let hex = s.trim().trim_start_matches('#');
    let byte = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
    };
    match (hex.len(), byte(0), byte(2), byte(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok(Rgb(r, g, b)),
        _ => Err(format!(
            "not a color: {s:?} (use #rrggbb or a name like red, blue)"
        )),
    }
}

/// A send level as normalized or dB (sends top out at 0 dB); dB wins when both are given.
pub(crate) fn send_volume_of(
    volume: Option<f64>,
    volume_db: Option<f64>,
) -> Result<Option<f64>, McpError> {
    if let Some(db) = volume_db {
        if db.is_nan() {
            return Err(bad("volume_db is not a number"));
        }
        return Ok(Some(music::send_db_to_volume(db)));
    }
    match volume {
        Some(v) if !(0.0..=1.0).contains(&v) => {
            Err(bad("volume must be 0.0-1.0 (or use volume_db)"))
        }
        other => Ok(other),
    }
}

/// Volume as normalized or dB; dB wins when both are given.
pub(crate) fn volume_of(
    volume: Option<f64>,
    volume_db: Option<f64>,
) -> Result<Option<f64>, McpError> {
    if let Some(db) = volume_db {
        if db.is_nan() {
            return Err(bad("volume_db is not a number"));
        }
        return Ok(Some(music::db_to_volume(db)));
    }
    match volume {
        Some(v) if !(0.0..=1.0).contains(&v) => {
            Err(bad("volume must be 0.0-1.0 (or use volume_db)"))
        }
        other => Ok(other),
    }
}

pub(crate) fn unit(name: &str, v: Option<f64>) -> Result<Option<f64>, McpError> {
    match v {
        Some(x) if !(0.0..=1.0).contains(&x) => Err(bad(format!("{name} must be 0.0-1.0"))),
        other => Ok(other),
    }
}

/// Beats in a bar of a time signature like "4/4" or "6/8" (default 4).
pub(crate) fn beats_per_bar(signature: &str) -> f64 {
    let mut it = signature.split('/');
    let n: f64 = it.next().and_then(|n| n.trim().parse().ok()).unwrap_or(4.0);
    let d: f64 = it.next().and_then(|d| d.trim().parse().ok()).unwrap_or(4.0);
    if n > 0.0 && d > 0.0 { n * 4.0 / d } else { 4.0 }
}

/// A shaped 0-1 position for `curve`.
pub(crate) fn shaped(t: f64, curve: &str) -> f64 {
    match curve {
        "ease_in" => t * t,
        "ease_out" => 1.0 - (1.0 - t) * (1.0 - t),
        _ => t,
    }
}

/// The project without the detail: what is needed to decide the next call.
pub(crate) fn summarize_project(snap: &Json) -> Json {
    fn track(t: &Json) -> Json {
        let names = |k: &str| -> Vec<Json> {
            t[k].as_array()
                .map(|a| a.iter().map(|x| x["name"].clone()).collect())
                .unwrap_or_default()
        };
        let mut m = serde_json::Map::new();
        for k in ["index", "name", "type", "volume_text", "pan_text"] {
            m.insert(k.into(), t[k].clone());
        }
        for k in ["mute", "solo", "arm"] {
            if t[k] == true {
                m.insert(k.into(), true.into());
            }
        }
        m.insert("devices".into(), names("devices").into());
        let slots: Vec<Json> = t["clips"]
            .as_array()
            .map(|a| a.iter().map(|c| c["slot"].clone()).collect())
            .unwrap_or_default();
        m.insert("clip_slots".into(), slots.into());
        let sends: Vec<Json> = t["sends"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s["volume"].as_f64().is_some_and(|v| v > 0.0))
                    .map(|s| json!({"name": s["name"], "volume_text": s["volume_text"]}))
                    .collect()
            })
            .unwrap_or_default();
        if !sends.is_empty() {
            m.insert("sends".into(), sends.into());
        }
        Json::Object(m)
    }
    let list = |k: &str| -> Vec<Json> {
        snap[k]
            .as_array()
            .map(|a| a.iter().map(track).collect())
            .unwrap_or_default()
    };
    let mut o = snap.as_object().cloned().unwrap_or_default();
    o.insert("tracks".into(), list("tracks").into());
    o.insert("effect_tracks".into(), list("effect_tracks").into());
    if snap["master"].is_object() {
        o.insert("master".into(), track(&snap["master"]));
    }
    if let Some(sc) = snap["scenes"].as_array() {
        let sc: Vec<Json> = sc
            .iter()
            .map(|s| json!({"index": s["index"], "name": s["name"], "clip_count": s["clip_count"]}))
            .collect();
        o.insert("scenes".into(), sc.into());
    }
    o.insert("summary".into(), true.into());
    Json::Object(o)
}

/// Refuses to touch a "new" track whose type does not fit the kind asked for: renaming the
/// wrong track (once: the master) is worse than failing.
pub(crate) fn check_created_type(kind: TrackKind, ty: Option<&str>) -> Result<(), String> {
    let ok = match (kind, ty) {
        (_, None) => true, // older extension: no type reported
        (TrackKind::Instrument, Some(t)) => t == "Instrument" || t == "Hybrid",
        (TrackKind::Audio, Some(t)) => t == "Audio" || t == "Hybrid",
        (TrackKind::Effect, Some(t)) => t == "Effect",
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "Bitwig reported the new track as {:?}, not {kind:?}; nothing was changed. \
             Check get_project.",
            ty.unwrap_or_default()
        ))
    }
}

/// Index of `name` among `names` (exact, then unique substring), 0-based.
pub(crate) fn pick(names: &[String], name: &str, what: &str) -> Result<u32, McpError> {
    let n = name.trim().to_lowercase();
    if let Ok(num) = n.parse::<u32>() {
        return num
            .checked_sub(1)
            .filter(|i| (*i as usize) < names.len())
            .ok_or_else(|| bad(format!("{what} {num} does not exist")));
    }
    let idx = names
        .iter()
        .position(|p| p.to_lowercase() == n)
        .or_else(|| {
            let hits: Vec<usize> = names
                .iter()
                .enumerate()
                .filter(|(_, p)| p.to_lowercase().contains(&n))
                .map(|(i, _)| i)
                .collect();
            (hits.len() == 1).then(|| hits[0])
        })
        .ok_or_else(|| bad(format!("no unique {what} {name:?}; have {names:?}")))?;
    u32::try_from(idx).map_err(err)
}

/// The snapshot entry of a clip (Null when the slot is empty).
pub(crate) fn track_clip(snapshot: &Json, track: TrackAddr, slot: u32) -> Json {
    let list = match track.bank {
        crate::bridge::protocol::TrackBank::Main => &snapshot["tracks"],
        crate::bridge::protocol::TrackBank::Effect => &snapshot["effect_tracks"],
        crate::bridge::protocol::TrackBank::Master => return Json::Null,
    };
    list.as_array()
        .and_then(|l| l.iter().find(|t| t["index"] == track.index))
        .and_then(|t| t["clips"].as_array())
        .and_then(|c| c.iter().find(|c| c["slot"] == slot))
        .cloned()
        .unwrap_or(Json::Null)
}

/// A step or rate like "1/16", "1/8T", "0.5" in beats.
pub(crate) fn parse_step(text: Option<&str>, default: f64) -> Result<f64, McpError> {
    let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(default);
    };
    let (body, triplet) = match t.strip_suffix(['T', 't']) {
        Some(b) => (b, true),
        None => (t, false),
    };
    let beats = match body.split_once('/') {
        // "1/16" of a whole note = 4/16 beats
        Some((n, d)) => {
            let n: f64 = n
                .trim()
                .parse()
                .map_err(|_| bad(format!("bad step {t:?}")))?;
            let d: f64 = d
                .trim()
                .parse()
                .map_err(|_| bad(format!("bad step {t:?}")))?;
            4.0 * n / d
        }
        None => body.parse().map_err(|_| bad(format!("bad step {t:?}")))?,
    };
    let beats = if triplet { beats * 2.0 / 3.0 } else { beats };
    if !(beats.is_finite() && (1.0 / 64.0..=16.0).contains(&beats)) {
        return Err(bad(format!("step {t:?} must be between 1/256 and 4 bars")));
    }
    Ok(beats)
}

pub(crate) fn chords_of(
    key: Option<&str>,
    progression: &str,
) -> Result<Vec<theory::Chord>, McpError> {
    let scale = key.map(theory::Scale::parse).transpose().map_err(bad)?;
    let chords = theory::progression(progression, scale.as_ref()).map_err(bad)?;
    if chords.len() > 64 {
        return Err(bad("at most 64 chords"));
    }
    Ok(chords)
}

pub(crate) fn beats_of(per_chord: Option<f64>) -> Result<f64, McpError> {
    let b = per_chord.unwrap_or(4.0);
    if !(b.is_finite() && (0.25..=64.0).contains(&b)) {
        return Err(bad("beats_per_chord must be 0.25-64"));
    }
    Ok(b)
}

pub(crate) fn rhythm_of(text: Option<&str>) -> Result<Option<Vec<theory::Step>>, McpError> {
    text.map(theory::steps).transpose().map_err(bad)
}

/// Resolves a drum voice: a key name of the profile, a note name or a number.
pub(crate) fn voice_key(profile: Option<&Profile>, voice: &str) -> Result<u8, McpError> {
    if let Some(k) = profile.and_then(|p| p.key(voice)) {
        return Ok(k);
    }
    if let Ok(n) = voice.trim().parse::<u8>()
        && n <= 127
    {
        return Ok(n);
    }
    music::parse_note(voice).map_err(|_| {
        let names: Vec<&String> = profile.map(|p| p.keys.keys().collect()).unwrap_or_default();
        bad(format!(
            "unknown drum voice {voice:?}: use a key name {names:?}, a note name or a number"
        ))
    })
}

/// Resolves an export path: absolute stays, a bare name goes into the default folder; the
/// extension is set to `ext`.
pub(crate) fn export_path(path: Option<&str>, default_stem: &str, ext: &str) -> PathBuf {
    let p = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        Some(p) => capture::default_dir().join(p),
        None => capture::default_dir().join(capture::safe_name(default_stem)),
    };
    p.with_extension(ext)
}

/// Tempo and beats per bar from a snapshot.
pub(crate) fn tempo_of(snapshot: &Json) -> (f64, f64, u8, u8) {
    let tempo = snapshot["transport"]["tempo"]
        .as_f64()
        .unwrap_or(120.0)
        .max(20.0);
    let (num, den) = snapshot["transport"]["time_signature"]
        .as_str()
        .and_then(|s| s.split_once('/'))
        .and_then(|(n, d)| Some((n.trim().parse::<u8>().ok()?, d.trim().parse::<u8>().ok()?)))
        .unwrap_or((4, 4));
    (
        tempo,
        f64::from(num) * 4.0 / f64::from(den.max(1)),
        num,
        den,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_and_rates() {
        assert_eq!(parse_step(None, 0.25).ok(), Some(0.25));
        assert_eq!(parse_step(Some("1/16"), 1.0).ok(), Some(0.25));
        assert_eq!(parse_step(Some("1/8"), 1.0).ok(), Some(0.5));
        assert!((parse_step(Some("1/8T"), 1.0).expect("triplet") - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(parse_step(Some("2"), 1.0).ok(), Some(2.0));
        assert!(parse_step(Some("1/1024"), 1.0).is_err());
        assert!(parse_step(Some("fast"), 1.0).is_err());
    }

    #[test]
    fn drum_voices_resolve_names_notes_and_numbers() {
        let reg = Registry::with_builtin();
        let gm = reg.get("generic");
        assert_eq!(voice_key(gm, "kick").ok(), Some(36));
        assert_eq!(voice_key(gm, "Closed Hat").ok(), Some(42));
        assert_eq!(voice_key(gm, "C1").ok(), Some(36));
        assert_eq!(voice_key(None, "38").ok(), Some(38));
        assert!(
            voice_key(gm, "cowbell-ish")
                .unwrap_err()
                .message
                .contains("kick")
        );
    }

    #[test]
    fn bars_and_curves() {
        assert_eq!(beats_per_bar("4/4"), 4.0);
        assert_eq!(beats_per_bar("6/8"), 3.0);
        assert_eq!(beats_per_bar("nonsense"), 4.0);
        assert!(shaped(0.5, "ease_in") < 0.5 && shaped(0.5, "ease_out") > 0.5);
        assert_eq!(shaped(0.5, "linear"), 0.5);
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#ff8000"), Ok(Rgb(255, 128, 0)));
        assert_eq!(parse_color("00ff00"), Ok(Rgb(0, 255, 0)));
        assert_eq!(parse_color("Blue"), Ok(Rgb(0, 153, 217)));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("#gggggg").is_err());
        assert!(parse_color("chartreuse-ish").is_err());
    }

    #[test]
    fn resolutions() {
        // note values, like every other step argument: "1/16" is a sixteenth note
        assert_eq!(parse_step(Some("1/16"), 1.0).ok(), Some(0.25));
        assert!(parse_step(Some("1/0"), 1.0).is_err());
    }

    #[test]
    fn volumes() {
        assert_eq!(volume_of(Some(0.5), None).ok(), Some(Some(0.5)));
        let db0 = volume_of(Some(0.1), Some(0.0)).expect("db").expect("some");
        assert!((db0 - 0.7937).abs() < 1e-3, "dB wins");
        assert!(volume_of(Some(1.5), None).is_err());
        assert!(volume_of(None, Some(f64::NAN)).is_err());
    }

    #[test]
    fn created_track_type_is_checked() {
        assert!(check_created_type(TrackKind::Instrument, Some("Instrument")).is_ok());
        assert!(check_created_type(TrackKind::Instrument, Some("Hybrid")).is_ok());
        assert!(check_created_type(TrackKind::Audio, Some("Audio")).is_ok());
        assert!(check_created_type(TrackKind::Effect, Some("Effect")).is_ok());
        assert!(check_created_type(TrackKind::Instrument, Some("Master")).is_err());
        assert!(check_created_type(TrackKind::Audio, Some("Effect")).is_err());
        assert!(check_created_type(TrackKind::Instrument, None).is_ok());
    }

    #[test]
    fn picking_names() {
        let names = vec![
            "Main".to_owned(),
            "Filter".to_owned(),
            "Filter Env".to_owned(),
        ];
        assert_eq!(pick(&names, "filter", "page").ok(), Some(1));
        assert_eq!(pick(&names, "env", "page").ok(), Some(2));
        assert_eq!(pick(&names, "2", "page").ok(), Some(1));
        assert!(pick(&names, "9", "page").is_err());
        assert!(pick(&names, "x", "page").is_err());
    }

    #[test]
    fn project_summary_drops_the_detail() {
        let snap = json!({
            "application": {"project_name": "p"},
            "tracks": [{"index": 0, "name": "Bass", "type": "Instrument", "volume_text": "-6 dB",
                "pan_text": "0", "mute": true, "solo": false, "color": "#fff",
                "devices": [{"name": "Polysynth", "preset": "x", "enabled": true}],
                "clips": [{"slot": 2, "name": "a", "playing": false}],
                "sends": [{"name": "Reverb", "volume": 0.0}, {"name": "Delay", "volume": 0.5, "volume_text": "-3 dB"}]}],
            "effect_tracks": [],
            "master": {"name": "Master"},
            "scenes": [{"index": 0, "name": "Intro", "clip_count": 1, "color": "#000"}]
        });
        let s = summarize_project(&snap);
        let t = &s["tracks"][0];
        assert_eq!(t["devices"], json!(["Polysynth"]));
        assert_eq!(t["clip_slots"], json!([2]));
        assert_eq!(t["mute"], true);
        assert!(t.get("solo").is_none() && t.get("color").is_none());
        assert_eq!(
            t["sends"],
            json!([{"name": "Delay", "volume_text": "-3 dB"}])
        );
        assert!(s["scenes"][0].get("color").is_none());
        assert_eq!(s["application"]["project_name"], "p");
    }
}
