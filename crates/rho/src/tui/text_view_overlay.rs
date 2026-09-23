//! Read-only text panel opened on top of a picker.
//!
//! Shows one long text (an agent prompt, for example) in the single-pane
//! overlay chrome with scrolling. Enter, Esc, or q returns to the picker it was
//! opened from, with that picker's cursor intact. Feature code decides the
//! title and text; this module owns layout, keys, and the return path.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::{
    overlay_panel::{
        classify_panel_key, overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel,
        OverlayPanelFrame, PanelKey, PanelScroll, PanelScrollTarget,
    },
    render::wrap_text_lines,
    theme::Theme,
    App, ComposerMode, UiPicker,
};

const FOOTER: &str = "↑↓ scroll · PgUp/PgDn · Enter/Esc back";

#[derive(Debug)]
pub(super) struct TextViewOverlay {
    title: String,
    text: String,
    scroll: PanelScroll,
    /// Picker restored when the panel closes.
    parent: Box<UiPicker>,
}

impl App {
    /// Replaces the open picker with a read-only text panel. Closing the panel
    /// restores that picker. Without an open picker there is nothing to
    /// return to, so the composer is left as it was.
    pub(super) fn open_text_view_over_picker(&mut self, title: String, text: String) {
        let parent = match self.input_ui.take_composer() {
            ComposerMode::Picker(parent) => parent,
            other => {
                debug_assert!(false, "text view requires an open parent picker");
                self.input_ui.set_composer(other);
                return;
            }
        };
        self.set_status_quiet(title.clone());
        self.input_ui
            .set_composer(ComposerMode::TextView(Box::new(TextViewOverlay {
                title,
                text,
                scroll: PanelScroll::default(),
                parent: Box::new(parent),
            })));
    }

    pub(super) fn text_view_overlay_frame(&self, area: Rect) -> Option<OverlayPanelFrame> {
        let ComposerMode::TextView(overlay) = self.input_ui.composer() else {
            return None;
        };
        let lines = text_view_lines(&overlay.text, text_view_body_width(area));
        Some(render_overlay_panel(
            &overlay.title,
            FOOTER,
            &lines,
            overlay.scroll.offset(),
            area,
        ))
    }

    pub(super) fn scroll_text_view_overlay(
        &mut self,
        area: Rect,
        target: PanelScrollTarget,
    ) -> bool {
        let ComposerMode::TextView(overlay) = self.input_ui.composer_mut() else {
            return false;
        };
        let body_len = text_view_lines(&overlay.text, text_view_body_width(area)).len();
        let body_rows = overlay_panel_layout(area, body_len).body_rows;
        overlay.scroll.apply(target, body_len, body_rows);
        true
    }

    pub(super) fn clamp_text_view_overlay_scroll(&mut self, terminal: &ratatui::DefaultTerminal) {
        if let (ComposerMode::TextView(overlay), Ok(size)) =
            (self.input_ui.composer(), terminal.size())
        {
            let target = PanelScrollTarget::Absolute(overlay.scroll.offset());
            self.scroll_text_view_overlay(Rect::new(0, 0, size.width, size.height), target);
        }
    }

    pub(super) fn handle_text_view_overlay_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &ratatui::DefaultTerminal,
    ) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::TextView(_)) {
            return false;
        }
        match classify_panel_key(key) {
            PanelKey::Close => {
                self.close_text_view_overlay();
                true
            }
            PanelKey::Scroll(target) => {
                if let Ok(size) = terminal.size() {
                    self.scroll_text_view_overlay(Rect::new(0, 0, size.width, size.height), target);
                }
                true
            }
            PanelKey::Passthrough => false,
            PanelKey::Swallow => true,
        }
    }

    fn close_text_view_overlay(&mut self) {
        let ComposerMode::TextView(overlay) = self.input_ui.take_composer() else {
            return;
        };
        self.set_status_quiet(overlay.parent.title.clone());
        self.input_ui
            .set_composer(ComposerMode::Picker(*overlay.parent));
    }
}

fn text_view_body_width(area: Rect) -> usize {
    // Reserve the shared panel's scrollbar column before wrapping text.
    overlay_panel_inner_width(area).saturating_sub(1).max(1)
}

fn text_view_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::from(Span::styled("(empty)", Theme::dim()))];
    }
    wrap_text_lines(text, width, Theme::text())
}

#[cfg(test)]
#[path = "text_view_overlay_tests.rs"]
mod tests;
