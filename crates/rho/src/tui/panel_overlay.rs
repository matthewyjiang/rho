//! Dispatch for single-pane panel overlays ([`PanelOverlay`]).
//!
//! Each overlay module owns its content, keys, and scroll state. This module
//! is the one place that fans a composer-level event out to whichever panel
//! is open, so input, resize, and draw paths name panels once instead of
//! listing every variant.

use std::time::Instant;

use crossterm::event::{KeyEvent, MouseEventKind};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    overlay_panel::{OverlayPanelFrame, PanelScrollTarget},
    App, ComposerMode, PanelOverlay, HISTORY_MOUSE_SCROLL_LINES,
};

impl App {
    fn panel_overlay(&self) -> Option<&PanelOverlay> {
        match self.input_ui.composer() {
            ComposerMode::Panel(panel) => Some(panel),
            _ => None,
        }
    }

    /// Routes a key to the open panel. `false` when no panel is open or the
    /// panel passes the key through (Ctrl+C).
    pub(super) fn handle_panel_overlay_key(
        &mut self,
        key: KeyEvent,
        terminal: &DefaultTerminal,
    ) -> bool {
        match self.panel_overlay() {
            None => false,
            Some(PanelOverlay::Limits(_)) => self.handle_limits_overlay_key(key, terminal),
            Some(PanelOverlay::Doctor(_)) => self.handle_doctor_overlay_key(key, terminal),
            Some(PanelOverlay::Computer(_)) => self.handle_computer_overlay_key(key, terminal),
            Some(PanelOverlay::Hooks(_)) => self.handle_hooks_overlay_key(key, terminal),
            Some(PanelOverlay::TextView(_)) => self.handle_text_view_overlay_key(key, terminal),
            Some(PanelOverlay::Info(_)) => self.handle_info_overlay_key(key, terminal),
        }
    }

    /// Re-clamps the open panel's scroll after a resize.
    pub(super) fn clamp_panel_overlay_scroll(&mut self, terminal: &DefaultTerminal) {
        match self.panel_overlay() {
            None => {}
            Some(PanelOverlay::Limits(_)) => self.clamp_limits_overlay_scroll(terminal),
            Some(PanelOverlay::Doctor(_)) => self.clamp_doctor_overlay_scroll(terminal),
            Some(PanelOverlay::Computer(_)) => self.clamp_computer_overlay_scroll(terminal),
            Some(PanelOverlay::Hooks(_)) => self.clamp_hooks_overlay_scroll(terminal),
            Some(PanelOverlay::TextView(_)) => self.clamp_text_view_overlay_scroll(terminal),
            Some(PanelOverlay::Info(_)) => self.clamp_info_overlay_scroll(terminal),
        }
    }

    /// The open panel's frame at `area`, or `None` when no panel is open.
    pub(super) fn panel_overlay_frame(
        &self,
        area: Rect,
        now: Instant,
    ) -> Option<OverlayPanelFrame> {
        match self.panel_overlay()? {
            PanelOverlay::Limits(_) => self.limits_overlay_frame(area, now),
            PanelOverlay::Doctor(_) => self.doctor_overlay_frame(area, now),
            PanelOverlay::Computer(_) => self.computer_overlay_frame(area),
            PanelOverlay::Hooks(_) => self.hooks_overlay_frame(area),
            PanelOverlay::TextView(_) => self.text_view_overlay_frame(area),
            PanelOverlay::Info(_) => self.info_overlay_frame(area),
        }
    }

    /// Routes a pointer event to the open panel. `true` means a panel is open
    /// and consumed it: Info handles selection and copy; the rest scroll on
    /// the wheel and swallow clicks so controls hidden behind stay inert.
    pub(super) fn handle_panel_overlay_mouse(
        &mut self,
        kind: MouseEventKind,
        screen: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) -> bool {
        match self.panel_overlay() {
            None => return false,
            Some(PanelOverlay::Info(_)) => {
                self.handle_info_overlay_mouse(kind, screen, column, row, now);
                return true;
            }
            Some(_) => {}
        }
        self.clear_selections();
        self.clear_hovered_copy_buttons();
        self.clear_rail_pointer_state();
        self.history.set_scrollbar_drag(None);
        let lines = HISTORY_MOUSE_SCROLL_LINES as isize;
        let delta = match kind {
            MouseEventKind::ScrollUp => -lines,
            MouseEventKind::ScrollDown => lines,
            _ => return true,
        };
        self.scroll_panel_overlay(screen, PanelScrollTarget::Delta(delta));
        true
    }

    fn scroll_panel_overlay(&mut self, area: Rect, target: PanelScrollTarget) {
        match self.panel_overlay() {
            None => {}
            Some(PanelOverlay::Limits(_)) => {
                self.scroll_limits_overlay(area, target);
            }
            Some(PanelOverlay::Doctor(_)) => {
                self.scroll_doctor_overlay(area, target);
            }
            Some(PanelOverlay::Computer(_)) => {
                self.scroll_computer_overlay(area, target);
            }
            Some(PanelOverlay::Hooks(_)) => {
                self.scroll_hooks_overlay(area, target);
            }
            Some(PanelOverlay::TextView(_)) => {
                self.scroll_text_view_overlay(area, target);
            }
            Some(PanelOverlay::Info(_)) => {
                self.scroll_info_overlay(area, target);
            }
        }
    }
}
