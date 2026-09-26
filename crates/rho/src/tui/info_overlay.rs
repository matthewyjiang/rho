//! `/info` overlay: runtime, usage, and workspace details in one pane.
//!
//! The command paints the in-memory snapshot immediately. Claude, Cursor, and
//! the session tree fill in afterwards. Closing the overlay does not leave a
//! transcript block. `c` copies the whole report; a drag copies the selection
//! (shared panel pointer, see `panel_pointer`).

use std::time::Instant;

use ratatui::text::Line;

use super::{
    background_tasks::{TaskId, UiOutput},
    info_command::{info_copy_text, load_external_runtimes, runtime_info_lines, RuntimeInfo},
    overlay_panel::{PanelBody, PanelState},
    App, ComposerMode, PanelOverlay,
};

const TITLE: &str = "Info";
const FOOTER: &str = "c copy  Enter/Esc close";

#[derive(Clone, Debug)]
pub(super) struct InfoOverlay {
    info: RuntimeInfo,
    panel: PanelState,
}

impl PanelBody for InfoOverlay {
    fn state(&self) -> &PanelState {
        &self.panel
    }

    fn state_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn title(&self) -> &str {
        TITLE
    }

    fn footer(&self) -> &str {
        FOOTER
    }

    fn body_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        runtime_info_lines(&self.info, width)
    }

    fn copy_text(&self) -> Option<String> {
        Some(info_copy_text(&self.info))
    }

    fn close(self: Box<Self>, app: &mut App) {
        app.end_info_overlay();
    }
}

impl App {
    pub(super) fn show_info_overlay(&mut self, info: RuntimeInfo) {
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Info(Box::new(
                InfoOverlay {
                    info,
                    panel: PanelState::default(),
                },
            ))));
        self.set_status_quiet("info");
    }

    /// Spawn the slow reads after the overlay is already showing. Unit tests
    /// inject handles instead of launching children.
    pub(super) fn start_info_refresh(&mut self) {
        self.abort_info_refresh();
        if cfg!(test) {
            return;
        }
        self.tasks
            .spawn(TaskId::InfoRuntimes, load_external_runtimes(), |result| {
                UiOutput::InfoRuntimes(result).into()
            });
        if self.info_tree_loading() {
            let Some(session_id) = self.info.session.session_id.clone() else {
                return;
            };
            self.spawn_info_tree_read(session_id);
        }
    }

    fn spawn_info_tree_read(&mut self, session_id: String) {
        let cwd = self.info.runtime.cwd.clone();
        self.tasks.spawn_blocking(
            TaskId::InfoTree,
            move || crate::session::Session::tree_facts_by_id(&cwd, &session_id),
            |result| UiOutput::InfoTree(result).into(),
        );
    }

    /// Drop the refresh tasks. Called once the overlay has left the composer.
    fn end_info_overlay(&mut self) {
        self.info_tree_deferred = false;
        self.abort_info_refresh();
    }

    /// Approvals and other composer replacements do not go through close;
    /// drop leftover reads once the overlay is gone.
    pub(super) async fn cancel_orphaned_info_refresh(&mut self) {
        if !matches!(
            self.input_ui.composer(),
            ComposerMode::Panel(PanelOverlay::Info(_))
        ) {
            self.cancel_info_refresh().await;
        }
    }

    pub(super) fn apply_info_runtimes_result(
        &mut self,
        result: Result<Vec<String>, tokio::task::JoinError>,
    ) -> bool {
        let Ok(lines) = result else {
            return false;
        };
        self.apply_info_runtimes(lines)
    }

    pub(super) fn apply_info_tree_result(
        &mut self,
        result: Result<
            anyhow::Result<crate::session::tree::SessionTreeFacts>,
            tokio::task::JoinError,
        >,
    ) -> bool {
        match result {
            Ok(Ok(facts)) => self.apply_info_tree(Some(facts), None),
            Ok(Err(error)) => self.apply_info_tree(None, Some(error.to_string())),
            Err(_) => false,
        }
    }

    /// Stop the probes. The tree read is `spawn_blocking` and detaches: it
    /// finishes in the background without blocking cancel or shutdown.
    pub(super) async fn cancel_info_refresh(&mut self) {
        self.info_tree_deferred = false;
        self.tasks
            .cancel(|id| matches!(id, TaskId::InfoRuntimes | TaskId::InfoTree))
            .await;
    }

    /// Start the session-tree read skipped while a turn was writing the tree.
    pub(super) fn start_deferred_info_tree(&mut self) -> bool {
        if !self.info_tree_deferred || self.is_ui_busy() {
            return false;
        }
        self.info_tree_deferred = false;
        if !matches!(
            self.input_ui.composer(),
            ComposerMode::Panel(PanelOverlay::Info(_))
        ) {
            return false;
        }
        let Some(session_id) = self.info.session.session_id.clone() else {
            return false;
        };
        self.mark_info_tree_loading();
        if cfg!(test) {
            let _ = session_id;
            return true;
        }
        self.spawn_info_tree_read(session_id);
        true
    }

    fn abort_info_refresh(&mut self) {
        self.tasks
            .abort(|id| matches!(id, TaskId::InfoRuntimes | TaskId::InfoTree));
    }

    fn apply_info_runtimes(&mut self, lines: Vec<String>) -> bool {
        let ComposerMode::Panel(PanelOverlay::Info(overlay)) = self.input_ui.composer_mut() else {
            return false;
        };
        overlay.info.set_external_runtimes(lines);
        overlay.panel.pointer.clear_selection();
        true
    }

    fn apply_info_tree(
        &mut self,
        tree: Option<crate::session::tree::SessionTreeFacts>,
        error: Option<String>,
    ) -> bool {
        let ComposerMode::Panel(PanelOverlay::Info(overlay)) = self.input_ui.composer_mut() else {
            return false;
        };
        overlay.info.set_tree(tree, error);
        overlay.panel.pointer.clear_selection();
        true
    }

    fn mark_info_tree_loading(&mut self) {
        let ComposerMode::Panel(PanelOverlay::Info(overlay)) = self.input_ui.composer_mut() else {
            return;
        };
        overlay.info.begin_tree_load();
    }

    fn info_tree_loading(&self) -> bool {
        let ComposerMode::Panel(PanelOverlay::Info(overlay)) = self.input_ui.composer() else {
            return false;
        };
        overlay.info.tree_loading()
    }
}

#[cfg(test)]
#[path = "info_overlay_tests.rs"]
mod tests;
