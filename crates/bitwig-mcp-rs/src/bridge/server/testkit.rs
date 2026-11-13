//! A server on a fake bridge, for the tests of every module.

use super::*;
use crate::bridge::client::fake;

fn snapshot() -> Json {
    json!({
        "application": {"project_name": "Demo"},
        "transport": {"playing": false, "tempo": 666.0},
        "tracks": [
            {"bank": "main", "index": 0, "name": "Drums",
             "devices": [{"index": 0, "name": "Drum Machine"}],
             "sends": [{"index": 0, "name": "Reverb"}]},
            {"bank": "main", "index": 1, "name": "Synth",
             "devices": [{"index": 0, "name": "HW Instrument"}],
             "clips": [{"slot": 0, "name": "pad"}]}
        ],
        "effect_tracks": [{"bank": "effect", "index": 0, "name": "Reverb"}],
        "master": {"bank": "master", "index": 0, "name": "Master"},
        "scenes": [{"index": 0, "name": "Intro"}, {"index": 1, "name": "Verse"}]
    })
}

/// A server on a fake bridge that answers get_project with [`snapshot`] and echoes the rest.
pub(crate) async fn rig() -> (BridgeServer, Arc<std::sync::Mutex<Vec<Json>>>) {
    let (port, log) = fake::spawn(|req| match req["cmd"].as_str() {
        Some("get_project") => Ok(snapshot()),
        Some("create_track") => Ok(json!({"bank": "main", "index": 2})),
        Some("create_scene") => Ok(json!({"index": 2})),
        Some("read_notes") => Ok(json!({
            "clip": {"loop_length": 4.0},
            "notes": [
                {"x": 0, "key": 60, "velocity": 100, "duration": 0.5},
                {"x": 32, "key": 64, "velocity": 80, "duration": 0.5}
            ]
        })),
        Some("get_song_plan") => Ok(json!({"text": "{\"key\":\"A minor\"}"})),
        Some("get_device") => Ok(json!({
            "pages": ["Main", "Filter"],
            // a page with a gap, like Bitwig's Test Tone (parameters 1, 2 and 8)
            "params": [{"name": "Cutoff", "index": 0}, {"name": "Resonance", "index": 7}]
        })),
        Some("insert_device") if req["source"]["name"] == "Nope" => Err("not found".into()),
        _ => Ok(req.clone()),
    })
    .await;
    let dir = std::env::temp_dir().join("bwmcp-no-profiles");
    (
        BridgeServer::new(Arc::new(Bridge::new("127.0.0.1", port)), dir),
        log,
    )
}

pub(crate) fn sent(log: &std::sync::Mutex<Vec<Json>>, cmd: &str) -> Vec<Json> {
    log.lock()
        .expect("log")
        .iter()
        .filter(|r| r["cmd"] == cmd)
        .cloned()
        .collect()
}
