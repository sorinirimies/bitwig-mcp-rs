//! The parts of the bridge's project snapshot the Rust side needs, and name resolution:
//! turning what a user says ("Bass", 3, "fx 2", "master") into wire addresses. Pure.

use serde::Deserialize;
use serde_json::Value as Json;

use super::protocol::{TrackAddr, TrackBank};

/// A device on a track.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeviceView {
    /// 0-based position in the chain.
    pub index: u32,
    /// Device name.
    pub name: String,
}

/// A send of a track.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SendView {
    /// 0-based send.
    pub index: u32,
    /// Name (the effect track it feeds).
    pub name: String,
}

/// A track.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct TrackView {
    /// Which list.
    pub bank: TrackBank,
    /// 0-based position in the list.
    pub index: u32,
    /// Name.
    pub name: String,
    /// Devices in the chain.
    pub devices: Vec<DeviceView>,
    /// Sends.
    pub sends: Vec<SendView>,
}

impl Default for TrackView {
    fn default() -> Self {
        Self {
            bank: TrackBank::Main,
            index: 0,
            name: String::new(),
            devices: Vec::new(),
            sends: Vec::new(),
        }
    }
}

impl TrackView {
    /// Its wire address.
    #[must_use]
    pub fn addr(&self) -> TrackAddr {
        TrackAddr {
            bank: self.bank,
            index: self.index,
        }
    }
}

/// A scene.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SceneView {
    /// 0-based.
    pub index: u32,
    /// Name.
    pub name: String,
}

/// The snapshot, reduced.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProjectView {
    /// Main tracks (flattened).
    pub tracks: Vec<TrackView>,
    /// Effect tracks.
    pub effect_tracks: Vec<TrackView>,
    /// Scenes.
    pub scenes: Vec<SceneView>,
}

impl ProjectView {
    /// Reads the reduced view out of a full snapshot.
    ///
    /// # Errors
    /// When the snapshot does not have the expected shape.
    pub fn from_snapshot(v: &Json) -> Result<Self, String> {
        Self::deserialize(v).map_err(|e| format!("unexpected project snapshot: {e}"))
    }
}

/// How a user names something: a 1-based number or a name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Ref {
    /// 1-based position.
    Number(u32),
    /// Name (case-insensitive; exact match first, then a unique substring).
    Name(String),
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Name(s) => write!(f, "{s:?}"),
        }
    }
}

/// Picks one item by name: exact (case-insensitive) first, then a unique substring.
fn by_name<'a, T>(
    items: &'a [T],
    name: &str,
    get: impl Fn(&T) -> &str,
    what: &str,
) -> Result<&'a T, String> {
    let needle = name.trim().to_lowercase();
    if needle.is_empty() {
        return Err(format!("empty {what} name"));
    }
    if let Some(t) = items.iter().find(|t| get(t).to_lowercase() == needle) {
        return Ok(t);
    }
    let hits: Vec<&T> = items
        .iter()
        .filter(|t| get(t).to_lowercase().contains(&needle))
        .collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => {
            let names: Vec<&str> = items.iter().map(&get).collect();
            Err(format!("no {what} named {name:?}; have {names:?}"))
        }
        many => {
            let names: Vec<&str> = many.iter().map(|t| get(t)).collect();
            Err(format!("{what} {name:?} is ambiguous: {names:?}"))
        }
    }
}

fn by_number<'a, T>(items: &'a [T], n: u32, what: &str) -> Result<&'a T, String> {
    n.checked_sub(1)
        .and_then(|i| items.get(i as usize))
        .ok_or_else(|| format!("{what} {n} does not exist (have 1-{})", items.len()))
}

impl ProjectView {
    /// Resolves a track. Numbers count main tracks; names search main then effect tracks;
    /// `"master"` is the master track; `"fx N"` is the N-th effect track.
    ///
    /// # Errors
    /// Unknown or ambiguous references.
    pub fn track(&self, r: &Ref) -> Result<TrackAddr, String> {
        match r {
            Ref::Number(n) => by_number(&self.tracks, *n, "track").map(TrackView::addr),
            Ref::Name(name) => {
                let lower = name.trim().to_lowercase();
                if lower == "master" {
                    return Ok(TrackAddr {
                        bank: TrackBank::Master,
                        index: 0,
                    });
                }
                if let Ok(n) = lower.parse::<u32>() {
                    return self.track(&Ref::Number(n));
                }
                if let Some(n) = lower
                    .strip_prefix("fx")
                    .map(str::trim)
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    return by_number(&self.effect_tracks, n, "effect track").map(TrackView::addr);
                }
                let all: Vec<TrackView> = self
                    .tracks
                    .iter()
                    .chain(&self.effect_tracks)
                    .cloned()
                    .collect();
                by_name(&all, name, |t| t.name.as_str(), "track").map(TrackView::addr)
            }
        }
    }

    /// The view of a resolved track (master has none).
    #[must_use]
    pub fn track_view(&self, addr: TrackAddr) -> Option<&TrackView> {
        let list = match addr.bank {
            TrackBank::Main => &self.tracks,
            TrackBank::Effect => &self.effect_tracks,
            TrackBank::Master => return None,
        };
        list.iter().find(|t| t.index == addr.index)
    }

    /// Resolves a scene to its 0-based index.
    ///
    /// # Errors
    /// Unknown or ambiguous references.
    pub fn scene(&self, r: &Ref) -> Result<u32, String> {
        match r {
            Ref::Number(n) => by_number(&self.scenes, *n, "scene").map(|s| s.index),
            Ref::Name(name) => match name.trim().parse::<u32>() {
                Ok(n) => self.scene(&Ref::Number(n)),
                Err(_) => {
                    by_name(&self.scenes, name, |s| s.name.as_str(), "scene").map(|s| s.index)
                }
            },
        }
    }

    /// Resolves a clip slot: a 1-based number or a scene name.
    ///
    /// # Errors
    /// Unknown or ambiguous references; slots past the scene list are allowed by number.
    pub fn slot(&self, r: &Ref) -> Result<u32, String> {
        match r {
            Ref::Number(0) => Err("slot numbers start at 1".into()),
            Ref::Number(n) => Ok(n - 1),
            Ref::Name(_) => self.scene(r),
        }
    }

    /// Resolves a device on a track to its 0-based index.
    ///
    /// # Errors
    /// Unknown track or device.
    pub fn device(&self, track: TrackAddr, r: &Ref) -> Result<u32, String> {
        let devices = self
            .track_view(track)
            .map(|t| t.devices.as_slice())
            .unwrap_or_default();
        match r {
            Ref::Number(n) => by_number(devices, *n, "device").map(|d| d.index),
            Ref::Name(name) => match name.trim().parse::<u32>() {
                Ok(n) => self.device(track, &Ref::Number(n)),
                Err(_) => by_name(devices, name, |d| d.name.as_str(), "device").map(|d| d.index),
            },
        }
    }

    /// Resolves a send of a track to its 0-based index: number or effect-track name.
    ///
    /// # Errors
    /// Unknown sends.
    pub fn send(&self, track: TrackAddr, r: &Ref) -> Result<u32, String> {
        let sends = self
            .track_view(track)
            .map(|t| t.sends.as_slice())
            .unwrap_or_default();
        match r {
            Ref::Number(n) => by_number(sends, *n, "send").map(|s| s.index),
            Ref::Name(name) => by_name(sends, name, |s| s.name.as_str(), "send").map(|s| s.index),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectView {
        ProjectView::from_snapshot(&serde_json::json!({
            "tracks": [
                {"bank": "main", "index": 0, "name": "Drums",
                 "devices": [{"index": 0, "name": "Drum Machine"}, {"index": 1, "name": "EQ+"}],
                 "sends": [{"index": 0, "name": "Reverb"}]},
                {"bank": "main", "index": 1, "name": "Bass"},
                {"bank": "main", "index": 2, "name": "Bass 2"},
                {"bank": "main", "index": 3, "name": "Keys"},
            ],
            "effect_tracks": [{"bank": "effect", "index": 0, "name": "Reverb"}],
            "scenes": [{"index": 0, "name": "Intro"}, {"index": 1, "name": "Verse"}],
            "transport": {"playing": false}
        }))
        .expect("snapshot")
    }

    fn name(s: &str) -> Ref {
        Ref::Name(s.into())
    }

    #[test]
    fn tracks_by_number_name_and_specials() {
        let p = project();
        let main = |index| TrackAddr {
            bank: TrackBank::Main,
            index,
        };
        assert_eq!(p.track(&Ref::Number(2)), Ok(main(1)));
        assert_eq!(p.track(&name("2")), Ok(main(1)));
        assert_eq!(p.track(&name("bass")), Ok(main(1)), "exact beats substring");
        assert_eq!(p.track(&name("key")), Ok(main(3)));
        assert_eq!(
            p.track(&name("reverb")).map(|a| a.bank),
            Ok(TrackBank::Effect)
        );
        assert_eq!(
            p.track(&name("FX 1")).map(|a| a.bank),
            Ok(TrackBank::Effect)
        );
        assert_eq!(
            p.track(&name("Master")).map(|a| a.bank),
            Ok(TrackBank::Master)
        );
        assert!(p.track(&Ref::Number(0)).is_err());
        assert!(p.track(&Ref::Number(9)).is_err());
        assert!(p.track(&name("guitar")).unwrap_err().contains("Drums"));
        assert!(p.track(&name("as")).unwrap_err().contains("ambiguous"));
    }

    #[test]
    fn scenes_slots_devices_sends() {
        let p = project();
        assert_eq!(p.scene(&name("verse")), Ok(1));
        assert_eq!(p.scene(&Ref::Number(1)), Ok(0));
        assert_eq!(p.slot(&Ref::Number(7)), Ok(6));
        assert!(p.slot(&Ref::Number(0)).is_err());
        assert_eq!(p.slot(&name("Intro")), Ok(0));
        let drums = p.track(&Ref::Number(1)).expect("drums");
        assert_eq!(p.device(drums, &name("eq")), Ok(1));
        assert_eq!(p.device(drums, &Ref::Number(1)), Ok(0));
        assert!(p.device(drums, &Ref::Number(5)).is_err());
        assert_eq!(p.send(drums, &name("reverb")), Ok(0));
        let master = p.track(&name("master")).expect("master");
        assert!(p.device(master, &Ref::Number(1)).is_err());
    }

    #[test]
    fn refs_parse_from_json_numbers_and_strings() {
        let r: Ref = serde_json::from_str("3").expect("num");
        assert_eq!(r, Ref::Number(3));
        let r: Ref = serde_json::from_str("\"Bass\"").expect("str");
        assert_eq!(r, name("Bass"));
    }
}
