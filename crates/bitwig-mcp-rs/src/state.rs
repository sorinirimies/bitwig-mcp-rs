//! Cache of everything Bitwig reports, and the JSON views built from it. Pure.

use std::collections::BTreeMap;
use std::ops::Bound;

use rosc::{OscMessage, OscType};
use serde::Serialize;
use serde_json::{Value as Json, json};

use crate::protocol::Scale;

/// One reported value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    /// Integer (flags are 0/1, parameters are `0..=MAX_VALUE`).
    Int(i64),
    /// Float.
    Float(f64),
    /// Text, including colors as `rgb(r,g,b)`.
    Text(String),
    /// Message without arguments.
    Null,
}

impl Value {
    /// First argument of a message as a [`Value`].
    #[must_use]
    pub fn from_message(msg: &OscMessage) -> Self {
        match msg.args.first() {
            Some(OscType::Int(i)) => Self::Int(i64::from(*i)),
            Some(OscType::Long(i)) => Self::Int(*i),
            Some(OscType::Float(f)) => Self::Float(f64::from(*f)),
            Some(OscType::Double(f)) => Self::Float(*f),
            Some(OscType::String(s)) => Self::Text(s.clone()),
            Some(OscType::Bool(b)) => Self::Int(i64::from(*b)),
            Some(OscType::Color(c)) => Self::Text(format!("rgb({},{},{})", c.red, c.green, c.blue)),
            _ => Self::Null,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            #[allow(clippy::cast_precision_loss)]
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// Latest value per OSC address.
#[derive(Debug, Default, Clone)]
pub struct State {
    map: BTreeMap<String, Value>,
}

impl State {
    /// Records a message from Bitwig. Housekeeping (`/update`) is ignored.
    pub fn apply(&mut self, msg: &OscMessage) {
        if msg.addr == "/update" {
            return;
        }
        self.map.insert(msg.addr.clone(), Value::from_message(msg));
    }

    /// Number of known addresses.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True before Bitwig reported anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Value of one address.
    #[must_use]
    pub fn get(&self, addr: &str) -> Option<&Value> {
        self.map.get(addr)
    }

    /// All entries whose address starts with `prefix` (empty = everything).
    #[must_use]
    pub fn with_prefix(&self, prefix: &str) -> BTreeMap<&str, &Value> {
        self.map
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v))
            .collect()
    }

    fn text(&self, addr: &str) -> Json {
        match self.get(addr) {
            Some(Value::Text(s)) => json!(s),
            Some(Value::Int(i)) => json!(i),
            Some(Value::Float(f)) => json!(f),
            _ => Json::Null,
        }
    }

    fn flag(&self, addr: &str) -> Json {
        match self.get(addr).and_then(Value::as_f64) {
            Some(v) => json!(v >= 0.5),
            None => Json::Null,
        }
    }

    fn is_on(&self, addr: &str) -> bool {
        self.get(addr)
            .and_then(Value::as_f64)
            .is_some_and(|v| v >= 0.5)
    }

    fn level(&self, addr: &str, scale: Scale) -> Json {
        match self.get(addr).and_then(Value::as_f64) {
            Some(v) => json!((scale.from_wire(v) * 1000.0).round() / 1000.0),
            None => Json::Null,
        }
    }

    /// Wire value of a parameter as an integer, for display.
    fn raw(&self, addr: &str) -> Json {
        self.get(addr)
            .and_then(Value::as_f64)
            .map_or(Json::Null, |v| json!(v))
    }

    /// Transport and project facts.
    #[must_use]
    pub fn transport(&self) -> Json {
        json!({
            "project": self.text("/project/name"),
            "engine": self.flag("/project/engine"),
            "playing": self.flag("/play"),
            "recording": self.flag("/record"),
            "loop": self.flag("/repeat"),
            "metronome": self.flag("/click"),
            "tempo_bpm": self.text("/tempo/raw"),
            "time": self.text("/time/str"),
            "beats": self.text("/beat/str"),
            "time_signature": self.text("/time/signature"),
            "layout": self.text("/layout"),
            "automation_write_mode": self.text("/automationWriteMode"),
            "arranger_overdub": self.flag("/overdub"),
            "launcher_overdub": self.flag("/overdub/launcher"),
        })
    }

    /// Existing tracks of the current bank with mixer values and clip slots.
    /// Volume and pan are normalized `0..=1` (pan 0.5 = center).
    #[must_use]
    pub fn tracks(&self, scale: Scale, bank: u8) -> Json {
        let tracks: Vec<Json> = (1..=u32::from(bank))
            .filter(|t| self.is_on(&format!("/track/{t}/exists")))
            .map(|t| {
                let p = format!("/track/{t}");
                let clips: Vec<Json> = (1..=u32::from(bank))
                    .filter(|s| self.is_on(&format!("{p}/clip/{s}/hasContent")))
                    .map(|s| {
                        let c = format!("{p}/clip/{s}");
                        json!({
                            "slot": s,
                            "name": self.text(&format!("{c}/name")),
                            "playing": self.flag(&format!("{c}/isPlaying")),
                            "recording": self.flag(&format!("{c}/isRecording")),
                            "queued": self.flag(&format!("{c}/isPlayingQueued")),
                        })
                    })
                    .collect();
                let sends: Vec<Json> = (1..=u32::from(bank))
                    .filter(|s| self.get(&format!("{p}/send/{s}/name")).is_some())
                    .map(|s| {
                        json!({
                            "send": s,
                            "name": self.text(&format!("{p}/send/{s}/name")),
                            "volume": self.level(&format!("{p}/send/{s}/volume"), scale),
                            "volume_text": self.text(&format!("{p}/send/{s}/volumeStr")),
                        })
                    })
                    .collect();
                json!({
                    "index": t,
                    "name": self.text(&format!("{p}/name")),
                    "type": self.text(&format!("{p}/type")),
                    "selected": self.flag(&format!("{p}/selected")),
                    "activated": self.flag(&format!("{p}/activated")),
                    "volume": self.level(&format!("{p}/volume"), scale),
                    "volume_text": self.text(&format!("{p}/volumeStr")),
                    "pan": self.level(&format!("{p}/pan"), scale),
                    "pan_text": self.text(&format!("{p}/panStr")),
                    "mute": self.flag(&format!("{p}/mute")),
                    "solo": self.flag(&format!("{p}/solo")),
                    "record_arm": self.flag(&format!("{p}/recarm")),
                    "monitor": self.flag(&format!("{p}/monitor")),
                    "is_group": self.flag(&format!("{p}/isGroup")),
                    "color": self.text(&format!("{p}/color")),
                    "sends": sends,
                    "clips": clips,
                })
            })
            .collect();
        json!({
            "tracks": tracks,
            "master": {
                "volume": self.level("/master/volume", scale),
                "volume_text": self.text("/master/volumeStr"),
                "pan": self.level("/master/pan", scale),
            },
        })
    }

    /// Existing scenes of the current bank.
    #[must_use]
    pub fn scenes(&self, bank: u8) -> Json {
        let v: Vec<Json> = (1..=u32::from(bank))
            .filter(|s| self.is_on(&format!("/scene/{s}/exists")))
            .map(|s| {
                json!({
                    "index": s,
                    "name": self.text(&format!("/scene/{s}/name")),
                    "selected": self.flag(&format!("/scene/{s}/selected")),
                })
            })
            .collect();
        json!(v)
    }

    /// The cursor device: parameters (normalized), pages, siblings, layers.
    #[must_use]
    pub fn device(&self, scale: Scale, bank: u8) -> Json {
        let n = 1..=u32::from(bank);
        let params: Vec<Json> = n
            .clone()
            .filter(|i| self.is_on(&format!("/device/param/{i}/exists")))
            .map(|i| {
                let p = format!("/device/param/{i}");
                json!({
                    "index": i,
                    "name": self.text(&format!("{p}/name")),
                    "value": self.level(&format!("{p}/value"), scale),
                    "value_text": self.text(&format!("{p}/valueStr")),
                })
            })
            .collect();
        let pages: Vec<Json> = n
            .clone()
            .filter(|i| self.is_on(&format!("/device/page/{i}/exists")))
            .map(|i| {
                json!({
                    "index": i,
                    "name": self.text(&format!("/device/page/{i}/name")),
                    "selected": self.flag(&format!("/device/page/{i}/selected")),
                })
            })
            .collect();
        let siblings: Vec<Json> = n
            .clone()
            .filter(|i| self.is_on(&format!("/device/sibling/{i}/exists")))
            .map(|i| {
                json!({
                    "index": i,
                    "name": self.text(&format!("/device/sibling/{i}/name")),
                    "selected": self.flag(&format!("/device/sibling/{i}/selected")),
                    "bypassed": self.flag(&format!("/device/sibling/{i}/bypass")),
                })
            })
            .collect();
        let layers: Vec<Json> = n
            .filter(|i| self.is_on(&format!("/device/layer/{i}/exists")))
            .map(|i| {
                let p = format!("/device/layer/{i}");
                json!({
                    "index": i,
                    "name": self.text(&format!("{p}/name")),
                    "selected": self.flag(&format!("{p}/selected")),
                    "volume": self.level(&format!("{p}/volume"), scale),
                    "mute": self.flag(&format!("{p}/mute")),
                    "solo": self.flag(&format!("{p}/solo")),
                })
            })
            .collect();
        json!({
            "exists": self.flag("/device/exists"),
            "name": self.text("/device/name"),
            "bypassed": self.flag("/device/bypass"),
            "selected_page": self.text("/device/page/selected/name"),
            "parameters": params,
            "pages": pages,
            "siblings": siblings,
            "layers": layers,
        })
    }

    /// The device/preset browser: tab, filter columns, results.
    #[must_use]
    pub fn browser(&self) -> Json {
        let filters: Vec<Json> = (1..=6)
            .filter(|f| self.is_on(&format!("/browser/filter/{f}/exists")))
            .map(|f| {
                let items: Vec<Json> = (1..=16)
                    .filter(|i| self.is_on(&format!("/browser/filter/{f}/item/{i}/exists")))
                    .map(|i| {
                        let p = format!("/browser/filter/{f}/item/{i}");
                        json!({
                            "index": i,
                            "name": self.text(&format!("{p}/name")),
                            "hits": self.raw(&format!("{p}/hits")),
                            "selected": self.flag(&format!("{p}/isSelected")),
                        })
                    })
                    .collect();
                json!({
                    "column": f,
                    "name": self.text(&format!("/browser/filter/{f}/name")),
                    "items": items,
                })
            })
            .collect();
        let results: Vec<Json> = (1..=16)
            .filter(|i| self.is_on(&format!("/browser/result/{i}/exists")))
            .map(|i| {
                json!({
                    "index": i,
                    "name": self.text(&format!("/browser/result/{i}/name")),
                    "selected": self.flag(&format!("/browser/result/{i}/isSelected")),
                })
            })
            .collect();
        json!({
            "active": self.flag("/browser/isActive"),
            "tab": self.text("/browser/tab"),
            "filters": filters,
            "results": results,
        })
    }

    /// Everything above in one document.
    #[must_use]
    pub fn overview(&self, scale: Scale, bank: u8) -> Json {
        json!({
            "transport": self.transport(),
            "mixer": self.tracks(scale, bank),
            "scenes": self.scenes(bank),
            "device": self.device(scale, bank),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(addr: &str, arg: OscType) -> OscMessage {
        OscMessage {
            addr: addr.into(),
            args: vec![arg],
        }
    }

    fn sample() -> State {
        let mut s = State::default();
        for (a, v) in [
            ("/track/1/exists", 1),
            ("/track/1/volume", 127),
            ("/track/1/mute", 1),
            ("/track/1/clip/2/hasContent", 1),
            ("/track/2/exists", 0),
            ("/play", 1),
            ("/scene/1/exists", 1),
        ] {
            s.apply(&msg(a, OscType::Int(v)));
        }
        s.apply(&msg("/track/1/name", OscType::String("Bass".into())));
        s.apply(&msg("/tempo/raw", OscType::Float(120.0)));
        s
    }

    #[test]
    fn apply_keeps_latest_and_skips_update() {
        let mut s = sample();
        s.apply(&msg("/play", OscType::Int(0)));
        s.apply(&msg("/update", OscType::Int(1)));
        assert_eq!(s.get("/play"), Some(&Value::Int(0)));
        assert!(s.get("/update").is_none());
    }

    #[test]
    fn prefix_query_is_exact_on_the_prefix() {
        let s = sample();
        let hits = s.with_prefix("/track/1/");
        assert!(hits.keys().all(|k| k.starts_with("/track/1/")));
        assert!(hits.contains_key("/track/1/name"));
        assert!(!hits.contains_key("/track/2/exists"));
        assert_eq!(s.with_prefix("").len(), s.len());
        assert!(s.with_prefix("/nothing").is_empty());
    }

    #[test]
    fn tracks_view_lists_only_existing_tracks() {
        let v = sample().tracks(Scale::new(128), 8);
        let tracks = v["tracks"].as_array().expect("array");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["name"], "Bass");
        assert_eq!(tracks[0]["volume"], 1.0);
        assert_eq!(tracks[0]["mute"], true);
        assert_eq!(tracks[0]["clips"][0]["slot"], 2);
    }

    #[test]
    fn transport_view_reads_flags_and_text() {
        let v = sample().transport();
        assert_eq!(v["playing"], true);
        assert_eq!(v["tempo_bpm"], 120.0);
        assert!(v["recording"].is_null());
    }

    #[test]
    fn empty_state_gives_empty_views() {
        let s = State::default();
        assert!(s.is_empty());
        let o = s.overview(Scale::new(128), 8);
        assert_eq!(o["mixer"]["tracks"].as_array().map(Vec::len), Some(0));
        assert_eq!(o["scenes"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn colors_become_text() {
        let m = msg(
            "/track/1/color",
            OscType::Color(rosc::OscColor {
                red: 1,
                green: 2,
                blue: 3,
                alpha: 255,
            }),
        );
        assert_eq!(Value::from_message(&m), Value::Text("rgb(1,2,3)".into()));
    }
}
