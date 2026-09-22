//! `/info` overlay: runtime, usage, and workspace details in one pane.
//!
//! The command paints the in-memory snapshot immediately. Claude, Cursor, and
//! the session tree fill in afterwards. Closing the overlay does not leave a
//! transcript block. `c` copies the whole report; a drag copies the selection.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use super::{
    copy_interaction::{selection_position, selection_position_clamped},
    info_command::{info_copy_text, load_external_runtimes, runtime_info_lines, RuntimeInfo},
    overlay_panel::{
        classify_panel_key, overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel,
        OverlayPanelFrame, PanelKey, PanelScroll, PanelScrollTarget,
    },
    text_selection::TextSelection,
    App, ComposerMode,
};

const TITLE: &str = "Info";
const FOOTER: &str = "c copy  Enter/Esc close";

#[derive(Clone, Debug)]
pub(super) struct InfoOverlay {
    info: RuntimeInfo,
    scroll: PanelScroll,
    selection: Option<TextSelection>,
}

impl App {
    pub(super) fn show_info_overlay(&mut self, info: RuntimeInfo) {
        self.input_ui
            .set_composer(ComposerMode::Info(Box::new(InfoOverlay {
                info,
                scroll: PanelScroll::default(),
                selection: None,
            })));
        self.set_status_quiet("info");
    }

    /// Spawn the slow reads after the overlay is already showing. Unit tests
    /// inject handles instead of launching children.
    pub(super) fn start_info_refresh(&mut self) {
        self.abort_info_refresh();
        if cfg!(test) {
            return;
        }
        self.pending_info_runtimes = Some(tokio::spawn(load_external_runtimes()));
        if self.info_tree_loading() {
            let Some(session_id) = self.info.session.session_id.clone() else {
                return;
            };
            let cwd = self.info.runtime.cwd.clone();
            self.pending_info_tree = Some(tokio::task::spawn_blocking(move || {
                crate::session::Session::tree_facts_by_id(&cwd, &session_id)
            }));
        }
    }

    pub(super) fn info_overlay_frame(&self, area: Rect) -> Option<OverlayPanelFrame> {
        let ComposerMode::Info(overlay) = self.input_ui.composer() else {
            return None;
        };
        let lines = runtime_info_lines(&overlay.info, info_body_width(area));
        Some(render_overlay_panel(
            TITLE,
            FOOTER,
            &lines,
            overlay.scroll.offset(),
            area,
        ))
    }

    pub(super) fn info_text_selection(&self) -> Option<TextSelection> {
        let ComposerMode::Info(overlay) = self.input_ui.composer() else {
            return None;
        };
        overlay.selection
    }

    pub(super) fn scroll_info_overlay(&mut self, area: Rect, target: PanelScrollTarget) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::Info(_)) {
            return false;
        }
        let body_len = self.info_body_len(area);
        let body_rows = overlay_panel_layout(area, body_len).body_rows;
        if let ComposerMode::Info(overlay) = self.input_ui.composer_mut() {
            overlay.scroll.apply(target, body_len, body_rows);
        }
        true
    }

    pub(super) fn clamp_info_overlay_scroll(&mut self, terminal: &ratatui::DefaultTerminal) {
        if let (ComposerMode::Info(overlay), Ok(size)) = (self.input_ui.composer(), terminal.size())
        {
            let target = PanelScrollTarget::Absolute(overlay.scroll.offset());
            self.scroll_info_overlay(Rect::new(0, 0, size.width, size.height), target);
        }
    }

    pub(super) fn handle_info_overlay_key(
        &mut self,
        key: KeyEvent,
        terminal: &ratatui::DefaultTerminal,
    ) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::Info(_)) {
            return false;
        }
        if is_copy_key(key) {
            self.copy_info_report(Instant::now());
            return true;
        }
        match classify_panel_key(key) {
            PanelKey::Close => {
                self.close_info_overlay();
                true
            }
            PanelKey::Scroll(target) => {
                if let Ok(size) = terminal.size() {
                    self.scroll_info_overlay(Rect::new(0, 0, size.width, size.height), target);
                }
                true
            }
            PanelKey::Passthrough => false,
            PanelKey::Swallow => true,
        }
    }

    pub(super) fn handle_info_overlay_mouse(
        &mut self,
        kind: MouseEventKind,
        screen: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) {
        self.clear_selections();
        self.clear_hovered_copy_buttons();
        self.clear_rail_pointer_state();
        self.history.set_scrollbar_drag(None);
        let Some(frame) = self.info_overlay_frame(screen) else {
            return;
        };
        let body = frame.body();
        let scroll = frame.scroll();
        match kind {
            MouseEventKind::ScrollUp => {
                self.scroll_info_overlay(
                    screen,
                    PanelScrollTarget::Delta(-(super::HISTORY_MOUSE_SCROLL_LINES as isize)),
                );
            }
            MouseEventKind::ScrollDown => {
                self.scroll_info_overlay(
                    screen,
                    PanelScrollTarget::Delta(super::HISTORY_MOUSE_SCROLL_LINES as isize),
                );
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let selection =
                    selection_position(body, scroll, column, row).map(TextSelection::new);
                self.set_info_selection(selection);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(position) = selection_position_clamped(body, scroll, column, row) else {
                    return;
                };
                if let Some(selection) = self.info_selection_mut() {
                    selection.update(position);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.copy_info_selection(body, scroll, column, row, screen, now);
            }
            _ => {}
        }
    }

    pub(super) fn close_info_overlay(&mut self) {
        if matches!(self.input_ui.composer(), ComposerMode::Info(_)) {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        self.info_tree_deferred = false;
        self.abort_info_refresh();
    }

    pub(super) async fn poll_info_refresh(&mut self) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::Info(_)) {
            // Approvals and other composer replacements do not go through close.
            self.cancel_info_refresh().await;
            return Ok(false);
        }
        let mut changed = false;
        if self
            .pending_info_runtimes
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            if let Some(handle) = self.pending_info_runtimes.take() {
                if let Ok(lines) = handle.await {
                    self.apply_info_runtimes(lines);
                    changed = true;
                }
            }
        }
        if self
            .pending_info_tree
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            if let Some(handle) = self.pending_info_tree.take() {
                match handle.await {
                    Ok(Ok(facts)) => {
                        self.apply_info_tree(Some(facts), None);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.apply_info_tree(None, Some(error.to_string()));
                        changed = true;
                    }
                    Err(_) => {}
                }
            }
        }
        Ok(changed)
    }

    pub(super) async fn cancel_info_refresh(&mut self) {
        self.info_tree_deferred = false;
        if let Some(handle) = self.pending_info_runtimes.take() {
            handle.abort();
            let _ = handle.await;
        }
        // spawn_blocking does not observe abort. Awaiting it stalls the event
        // loop until the tree read finishes. Drop the handle so the read can
        // finish in the background without blocking cancel or shutdown.
        if let Some(handle) = self.pending_info_tree.take() {
            handle.abort();
        }
    }

    /// Start the session-tree read skipped while a turn was writing the tree.
    pub(super) fn start_deferred_info_tree(&mut self) -> bool {
        if !self.info_tree_deferred || self.is_ui_busy() {
            return false;
        }
        self.info_tree_deferred = false;
        if !matches!(self.input_ui.composer(), ComposerMode::Info(_)) {
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
        let cwd = self.info.runtime.cwd.clone();
        self.pending_info_tree = Some(tokio::task::spawn_blocking(move || {
            crate::session::Session::tree_facts_by_id(&cwd, &session_id)
        }));
        true
    }

    fn abort_info_refresh(&mut self) {
        if let Some(handle) = self.pending_info_runtimes.take() {
            handle.abort();
        }
        if let Some(handle) = self.pending_info_tree.take() {
            handle.abort();
        }
    }

    fn copy_info_report(&mut self, now: Instant) {
        let ComposerMode::Info(overlay) = self.input_ui.composer() else {
            return;
        };
        let text = info_copy_text(&overlay.info);
        self.copy_text(&text, now);
    }

    fn copy_info_selection(
        &mut self,
        body: Rect,
        scroll: usize,
        column: u16,
        row: u16,
        screen: Rect,
        now: Instant,
    ) {
        let Some(mut selection) = self.take_info_selection() else {
            return;
        };
        if let Some(position) = selection_position_clamped(body, scroll, column, row) {
            selection.update(position);
        }
        if !selection.has_moved() {
            return;
        }
        let lines = self.info_body_lines(screen);
        let Some(text) = selection.selected_text(&lines, 0) else {
            return;
        };
        self.copy_text(&text, now);
        self.set_info_selection(Some(selection));
    }

    fn apply_info_runtimes(&mut self, lines: Vec<String>) {
        let ComposerMode::Info(overlay) = self.input_ui.composer_mut() else {
            return;
        };
        overlay.info.set_external_runtimes(lines);
        overlay.selection = None;
    }

    fn apply_info_tree(
        &mut self,
        tree: Option<crate::session::tree::SessionTreeFacts>,
        error: Option<String>,
    ) {
        let ComposerMode::Info(overlay) = self.input_ui.composer_mut() else {
            return;
        };
        overlay.info.set_tree(tree, error);
        overlay.selection = None;
    }

    fn mark_info_tree_loading(&mut self) {
        let ComposerMode::Info(overlay) = self.input_ui.composer_mut() else {
            return;
        };
        overlay.info.begin_tree_load();
    }

    fn info_tree_loading(&self) -> bool {
        let ComposerMode::Info(overlay) = self.input_ui.composer() else {
            return false;
        };
        overlay.info.tree_loading()
    }

    fn set_info_selection(&mut self, selection: Option<TextSelection>) {
        if let ComposerMode::Info(overlay) = self.input_ui.composer_mut() {
            overlay.selection = selection;
        }
    }

    fn info_selection_mut(&mut self) -> Option<&mut TextSelection> {
        let ComposerMode::Info(overlay) = self.input_ui.composer_mut() else {
            return None;
        };
        overlay.selection.as_mut()
    }

    fn take_info_selection(&mut self) -> Option<TextSelection> {
        let ComposerMode::Info(overlay) = self.input_ui.composer_mut() else {
            return None;
        };
        overlay.selection.take()
    }

    fn info_body_len(&self, area: Rect) -> usize {
        self.info_body_lines(area).len()
    }

    fn info_body_lines(&self, area: Rect) -> Vec<ratatui::text::Line<'static>> {
        let ComposerMode::Info(overlay) = self.input_ui.composer() else {
            return Vec::new();
        };
        runtime_info_lines(&overlay.info, info_body_width(area))
    }
}

fn info_body_width(area: Rect) -> usize {
    // Reserve the shared panel's scrollbar column before wrapping text.
    overlay_panel_inner_width(area).saturating_sub(1)
}

fn is_copy_key(key: KeyEvent) -> bool {
    matches!(
        (key.modifiers, key.code),
        (
            KeyModifiers::NONE | KeyModifiers::SHIFT,
            KeyCode::Char('c' | 'C')
        )
    )
}

#[cfg(test)]
#[path = "info_overlay_tests.rs"]
mod tests;
