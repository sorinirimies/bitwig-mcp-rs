//! Reading the project, meters, history, views and Bitwig's own actions.

use super::super::*;

/// What `get_project` returns.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ProjectArgs {
    /// true: every mixer value, send, device and clip in full. Default: a summary
    /// (name, type, volume, pan, mute/solo, device names, clip slots); use `get_track` for one
    /// track in full.
    pub detail: Option<bool>,
}

/// A track.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackArgs {
    /// Track name or 1-based number; "master"; "fx N" for effect tracks.
    pub track: Ref,
}

/// Action search.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListActionsArgs {
    /// Words to search in action ids and names, e.g. "save", "export", "group".
    pub query: String,
}

/// Action to run.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InvokeActionArgs {
    /// Action id from list_actions.
    pub id: String,
}

/// Undo or redo.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAction {
    /// Undo.
    Undo,
    /// Redo.
    Redo,
}

/// History.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HistoryArgs {
    /// undo or redo.
    pub action: HistoryAction,
    /// How many steps (default 1, max 50).
    pub steps: Option<u32>,
}

/// Window changes.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetViewArgs {
    /// "arrange", "mix" or "edit".
    pub layout: Option<String>,
    /// Panel to toggle: inspector, devices, mixer, note_editor, automation_editor, browser,
    /// fullscreen.
    pub toggle: Option<String>,
}

/// Bitwig's own export dialogs.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    /// Export Audio (offline render, stems, formats).
    Audio,
    /// Export MIDI.
    Midi,
    /// Export DAWproject (for other DAWs).
    Dawproject,
}

/// Which dialog.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportDialogArgs {
    /// audio, midi or dawproject.
    pub kind: ExportKind,
}

/// Levels.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct MetersArgs {
    /// Measure for this many milliseconds (default 0 = the peaks since the last read). Start
    /// playback first, e.g. launch a scene, and measure a bar or two.
    pub window_ms: Option<u32>,
}

#[tool_router(router = studio_router, vis = "pub(crate)")]
impl BridgeServer {
    #[tool(
        description = "Connection status: server version, bridge address, extension and protocol version, project name."
    )]
    pub(crate) async fn status(&self) -> ToolResult {
        match self.call(Command::GetProject).await {
            Ok(snap) => out(&json!({
                "connected": true,
                "server_version": env!("CARGO_PKG_VERSION"),
                "bridge": self.bridge.addr(),
                "extension": self.bridge.hello(),
                "project": snap["application"]["project_name"],
            })),
            Err(e) => out(&json!({
                "connected": false,
                "server_version": env!("CARGO_PKG_VERSION"),
                "bridge": self.bridge.addr(),
                "error": e.message,
            })),
        }
    }

    #[tool(
        description = "The project: transport, scenes and every track, effect track and the master. A summary by default (volume, pan, mute/solo, device names, clip slots); detail:true for everything. Read this first."
    )]
    pub(crate) async fn get_project(&self, Parameters(a): Parameters<ProjectArgs>) -> ToolResult {
        let snap = self.project().await?.0;
        if a.detail.unwrap_or(false) {
            out(&snap)
        } else {
            out(&summarize_project(&snap))
        }
    }

    #[tool(
        description = "One track in full: mixer values with display text, sends, devices and clips."
    )]
    pub(crate) async fn get_track(&self, Parameters(a): Parameters<TrackArgs>) -> ToolResult {
        let (snap, view) = self.project().await?;
        let addr = view.track(&a.track).map_err(bad)?;
        let list = match addr.bank {
            crate::bridge::protocol::TrackBank::Main => &snap["tracks"],
            crate::bridge::protocol::TrackBank::Effect => &snap["effect_tracks"],
            crate::bridge::protocol::TrackBank::Master => return out(&snap["master"]),
        };
        let t = list
            .as_array()
            .and_then(|l| l.iter().find(|t| t["index"] == addr.index))
            .ok_or_else(|| err("track vanished"))?;
        out(t)
    }

    #[tool(
        description = "Peak levels (0-1 on Bitwig's meter scale) of every track and the master. With window_ms the meters are cleared and read after that time: start playback, measure a bar or two, see which track is loud. For LUFS and true peak of the whole mix use capture_audio."
    )]
    pub(crate) async fn get_meters(&self, Parameters(a): Parameters<MetersArgs>) -> ToolResult {
        let ms = a.window_ms.unwrap_or(0);
        if ms > 60_000 {
            return Err(bad("window_ms must be 0-60000"));
        }
        let r = self
            .bridge
            .call_with(
                Command::GetMeters { window_ms: ms },
                Duration::from_millis(u64::from(ms) + 10_000),
            )
            .await
            .map_err(err)?;
        out(&r)
    }

    #[tool(
        description = "Search Bitwig's application actions (every menu command: save, export, group tracks, consolidate, ...) by words in their id or name."
    )]
    pub(crate) async fn list_actions(
        &self,
        Parameters(a): Parameters<ListActionsArgs>,
    ) -> ToolResult {
        out(&self.call(Command::ListActions { query: a.query }).await?)
    }

    #[tool(
        description = "Invoke a Bitwig application action by id (from list_actions). Acts on the current selection, like the menu."
    )]
    pub(crate) async fn invoke_action(
        &self,
        Parameters(a): Parameters<InvokeActionArgs>,
    ) -> ToolResult {
        out(&self.call(Command::InvokeAction { id: a.id }).await?)
    }

    #[tool(description = "Undo or redo one or more steps.")]
    pub(crate) async fn history(&self, Parameters(a): Parameters<HistoryArgs>) -> ToolResult {
        let steps = a.steps.unwrap_or(1).clamp(1, 50);
        for _ in 0..steps {
            let c = match a.action {
                HistoryAction::Undo => Command::Undo,
                HistoryAction::Redo => Command::Redo,
            };
            self.call(c).await?;
        }
        out(&json!({"done": steps}))
    }

    #[tool(
        description = "Switch the window layout (arrange, mix, edit) and/or toggle a panel (inspector, devices, mixer, note_editor, automation_editor, browser, fullscreen)."
    )]
    pub(crate) async fn set_view(&self, Parameters(a): Parameters<SetViewArgs>) -> ToolResult {
        let layout = match a.layout.as_deref().map(str::to_lowercase).as_deref() {
            None => None,
            Some(l @ ("arrange" | "mix" | "edit")) => Some(l.to_uppercase()),
            Some(o) => {
                return Err(bad(format!(
                    "layout must be arrange, mix or edit, not {o:?}"
                )));
            }
        };
        out(&self
            .call(Command::SetView {
                layout,
                toggle: a.toggle,
            })
            .await?)
    }

    #[tool(
        description = "Save the project (Bitwig's Save; an unsaved project asks for a name in Bitwig)."
    )]
    pub(crate) async fn save_project(&self) -> ToolResult {
        out(&self
            .call(Command::InvokeAction { id: "Save".into() })
            .await?)
    }

    #[tool(
        description = "Open one of Bitwig's export dialogs for the user to finish: audio (offline render with all options), midi or dawproject (for other DAWs)."
    )]
    pub(crate) async fn open_export_dialog(
        &self,
        Parameters(a): Parameters<ExportDialogArgs>,
    ) -> ToolResult {
        let id = match a.kind {
            ExportKind::Audio => "Export Audio",
            ExportKind::Midi => "export_midi",
            ExportKind::Dawproject => "export_project",
        };
        out(&self.call(Command::InvokeAction { id: id.into() }).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::server::testkit::{rig, sent};

    #[tokio::test]
    async fn get_meters_passes_the_window() {
        let (s, log) = rig().await;
        s.get_meters(Parameters(MetersArgs {
            window_ms: Some(50),
        }))
        .await
        .expect("tool");
        assert_eq!(sent(&log, "get_meters")[0]["window_ms"], 50);
    }

    #[tokio::test]
    async fn status_without_bitwig_says_so() {
        let s = BridgeServer::new(Arc::new(Bridge::new("127.0.0.1", 1)), PathBuf::new());
        let r = s.status().await.expect("status never fails");
        assert!(r.contains("\"connected\":false"), "{r}");
    }
}
