//! Dispatch for single-pane panel overlays ([`PanelOverlay`]).
//!
//! Each overlay implements [`PanelBody`]: it owns its content and feature
//! keys. This module owns everything shared, through one generic path: frame
//! building, scroll clamping, pointer input, the copy key, and the
//! close/scroll key table. Adding a panel means one [`PanelOverlay`] variant
//! and one arm in [`PanelOverlay::body`] / [`PanelOverlay::body_mut`] /
//! [`PanelOverlay::into_body`].

use std::time::Instant;

use crossterm::event::{KeyEvent, MouseEventKind};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    overlay_panel::{
        classify_panel_key, is_copy_key, panel_frame, scroll_panel, terminal_area,
        OverlayPanelFrame, PanelBody, PanelKey, PanelKeyOutcome, PanelScrollTarget,
    },
    panel_pointer::{PanelPointer, PanelPointerEffect, PanelPointerEvent},
    App, ComposerMode, PanelOverlay,
};

impl PanelOverlay {
    fn body(&self) -> &dyn PanelBody {
        match self {
            Self::Limits(overlay) => overlay,
            Self::Doctor(overlay) => overlay,
            Self::Computer(overlay) => overlay,
            Self::Hooks(overlay) => overlay,
            Self::TextView(overlay) => overlay.as_ref(),
            Self::Info(overlay) => overlay.as_ref(),
            Self::Spend(overlay) => overlay.as_ref(),
        }
    }

    fn body_mut(&mut self) -> &mut dyn PanelBody {
        match self {
            Self::Limits(overlay) => overlay,
            Self::Doctor(overlay) => overlay,
            Self::Computer(overlay) => overlay,
            Self::Hooks(overlay) => overlay,
            Self::TextView(overlay) => overlay.as_mut(),
            Self::Info(overlay) => overlay.as_mut(),
            Self::Spend(overlay) => overlay.as_mut(),
        }
    }

    /// Owned body, so [`PanelBody::close`] can move state out of the panel.
    fn into_body(self) -> Box<dyn PanelBody> {
        match self {
            Self::Limits(overlay) => Box::new(overlay),
            Self::Doctor(overlay) => Box::new(overlay),
            Self::Computer(overlay) => Box::new(overlay),
            Self::Hooks(overlay) => Box::new(overlay),
            Self::TextView(overlay) => overlay,
            Self::Info(overlay) => overlay,
            Self::Spend(overlay) => overlay,
        }
    }

    /// Pointer state of the open panel, for painting hover and selection.
    pub(super) fn pointer(&self) -> PanelPointer {
        self.body().state().pointer
    }
}

impl App {
    fn panel_overlay(&self) -> Option<&PanelOverlay> {
        match self.input_ui.composer() {
            ComposerMode::Panel(panel) => Some(panel),
            _ => None,
        }
    }

    fn panel_overlay_mut(&mut self) -> Option<&mut PanelOverlay> {
        match self.input_ui.composer_mut() {
            ComposerMode::Panel(panel) => Some(panel),
            _ => None,
        }
    }

    /// Routes a key to the open panel: its feature keys first, then copy,
    /// then the shared close/scroll table. `false` when no panel is open or
    /// the panel passes the key through (Ctrl+C).
    pub(super) fn handle_panel_overlay_key(
        &mut self,
        key: KeyEvent,
        terminal: &DefaultTerminal,
    ) -> bool {
        let Some(panel) = self.panel_overlay_mut() else {
            return false;
        };
        match panel.body_mut().handle_key(key) {
            PanelKeyOutcome::Handled => return true,
            PanelKeyOutcome::Run(action) => {
                action(self);
                return true;
            }
            PanelKeyOutcome::Unhandled => {}
        }
        if is_copy_key(key) {
            self.copy_panel_overlay(Instant::now());
            return true;
        }
        match classify_panel_key(key) {
            PanelKey::Close => {
                self.close_panel_overlay();
                true
            }
            PanelKey::Scroll(target) => {
                if let Some(area) = terminal_area(terminal) {
                    self.scroll_panel_overlay(area, target);
                }
                true
            }
            PanelKey::Passthrough => false,
            PanelKey::Swallow => true,
        }
    }

    /// Copies the open panel's report, when it has one, as plain text.
    pub(super) fn copy_panel_overlay(&mut self, now: Instant) {
        if let Some(text) = self
            .panel_overlay()
            .and_then(|panel| panel.body().copy_text())
        {
            self.copy_text(&text, now);
        }
    }

    /// Closes the open panel and runs its close policy. No-op without one.
    pub(super) fn close_panel_overlay(&mut self) {
        match self.input_ui.take_composer() {
            ComposerMode::Panel(panel) => panel.into_body().close(self),
            other => self.input_ui.set_composer(other),
        }
    }

    /// Re-clamps the open panel's scroll after a resize.
    pub(super) fn clamp_panel_overlay_scroll(&mut self, terminal: &DefaultTerminal) {
        let (Some(panel), Some(area)) = (self.panel_overlay(), terminal_area(terminal)) else {
            return;
        };
        let offset = panel.body().state().scroll.offset();
        self.scroll_panel_overlay(area, PanelScrollTarget::Absolute(offset));
    }

    /// The open panel's frame at `area`, or `None` when no panel is open.
    pub(super) fn panel_overlay_frame(
        &self,
        area: Rect,
        now: Instant,
    ) -> Option<OverlayPanelFrame> {
        Some(panel_frame(self.panel_overlay()?.body(), area, now))
    }

    /// Pointer input while a panel is open: wheel and scrollbar drag scroll
    /// the body, a drag selects and copies text, and copy targets copy on
    /// press. The panel owns every event so controls hidden behind it stay
    /// inert. Motion needs no work here: paint resolves hover from the app's
    /// last pointer cell.
    pub(super) fn handle_panel_overlay_mouse(
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
        // Building the frame renders the whole body; skip it for motion.
        let Some(event) = PanelPointerEvent::from_kind(kind) else {
            return;
        };
        // Hit-test against the frame the user sees, then mutate the panel.
        let Some(frame) = self.panel_overlay_frame(screen, now) else {
            return;
        };
        let Some(panel) = self.panel_overlay_mut() else {
            return;
        };
        let effect = panel
            .body_mut()
            .state_mut()
            .pointer
            .handle(event, column, row, &frame);
        match effect {
            PanelPointerEffect::None => {}
            PanelPointerEffect::ScrollTo(line) => {
                self.scroll_panel_overlay(screen, PanelScrollTarget::Absolute(line));
            }
            PanelPointerEffect::ScrollBy(delta) => {
                self.scroll_panel_overlay(screen, PanelScrollTarget::Delta(delta));
            }
            PanelPointerEffect::Copy(text) => self.copy_text(&text, now),
        }
    }

    fn scroll_panel_overlay(&mut self, area: Rect, target: PanelScrollTarget) {
        if let Some(panel) = self.panel_overlay_mut() {
            scroll_panel(panel.body_mut(), area, target);
        }
    }
}
