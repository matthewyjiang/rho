//! Read-only text panel opened on top of a picker.
//!
//! Shows one long text (an agent prompt, for example) in the single-pane
//! overlay chrome with scrolling. Enter, Esc, or q returns to the picker it was
//! opened from, with that picker's cursor intact. Feature code decides the
//! title and text; this module owns layout, keys, and the return path.

use std::time::Instant;

use ratatui::text::{Line, Span};

use super::{
    overlay_panel::{PanelBody, PanelState},
    render::wrap_text_lines,
    theme::Theme,
    App, ComposerMode, PanelOverlay, UiPicker,
};

const FOOTER: &str = "↑↓ scroll · PgUp/PgDn · Enter/Esc back";

#[derive(Debug)]
pub(super) struct TextViewOverlay {
    title: String,
    text: String,
    panel: PanelState,
    /// Picker restored when the panel closes.
    parent: Box<UiPicker>,
}

impl PanelBody for TextViewOverlay {
    fn state(&self) -> &PanelState {
        &self.panel
    }

    fn state_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn footer(&self) -> &str {
        FOOTER
    }

    fn body_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        text_view_lines(&self.text, width.max(1))
    }

    /// Returns to the picker the panel was opened from.
    fn close(self: Box<Self>, app: &mut App) {
        app.set_status_quiet(self.parent.title.clone());
        app.input_ui
            .set_composer(ComposerMode::Picker(*self.parent));
    }
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
            .set_composer(ComposerMode::Panel(PanelOverlay::TextView(Box::new(
                TextViewOverlay {
                    title,
                    text,
                    panel: PanelState::default(),
                    parent: Box::new(parent),
                },
            ))));
    }
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
