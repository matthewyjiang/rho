//! Live computer dashboard with immediate revocation, never implicit grants.

use ratatui::{layout::Rect, text::Line};

use super::{
    overlay_panel::{
        classify_panel_key, overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel,
        OverlayPanelFrame, PanelKey, PanelScroll, PanelScrollTarget,
    },
    panel_text::{heading_with_status, indented_wrapped_lines},
    theme::Theme,
    App, ComposerMode,
};
use crate::tools::computer_use::{desktop_warning, ComputerUseControl, ComputerUseStatus};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ComputerOverlay {
    scroll: PanelScroll,
}

impl App {
    pub(super) fn show_computer_status(&mut self) {
        self.input_ui
            .set_composer(ComposerMode::Computer(ComputerOverlay::default()));
    }

    fn computer_status_lines(&self, width: usize) -> Vec<Line<'static>> {
        let state = self
            .computer_use
            .as_ref()
            .map(ComputerUseControl::status)
            .unwrap_or(ComputerUseStatus::Off);
        let (status, access, next) = match state {
            ComputerUseStatus::Installing => (
                "Installing",
                "Cua Driver installation pending; desktop access remains off",
                "r or /computer off cancels installation",
            ),
            ComputerUseStatus::Off => (
                "Off",
                "No desktop access granted",
                "/computer on  grant access and connect",
            ),
            ComputerUseStatus::Connecting => (
                "Connecting",
                "Desktop access granted; connecting to driver",
                "/computer off  cancel connection and revoke access",
            ),
            ComputerUseStatus::Connected => (
                "Driver connected",
                "Desktop access granted for this session",
                "/computer off  revoke access and disconnect",
            ),
            ComputerUseStatus::Closing => (
                "Access off · disconnecting",
                "Access revoked; driver connection is closing",
                "Wait for closing to finish before /computer on",
            ),
        };
        let mut lines = vec![heading_with_status("Session", status, width)];
        lines.extend(indented_wrapped_lines(access, 0, width, Theme::text()));
        lines.extend(indented_wrapped_lines(next, 0, width, Theme::warning()));
        if matches!(
            state,
            ComputerUseStatus::Connecting | ComputerUseStatus::Connected
        ) {
            lines.extend(indented_wrapped_lines(
                "r revoke access now",
                0,
                width,
                Theme::warning(),
            ));
        }
        lines.extend(indented_wrapped_lines(
            "Desktop permissions: not checked",
            0,
            width,
            Theme::dim(),
        ));
        if let Some(reason) = self
            .computer_use
            .as_ref()
            .and_then(ComputerUseControl::revocation_reason)
        {
            lines.extend(indented_wrapped_lines(
                &format!("Access revoked: {reason}"),
                0,
                width,
                Theme::warning(),
            ));
        }
        if let Some(warning) = desktop_warning() {
            lines.push(Line::default());
            lines.push(heading_with_status("Display warning", "", width));
            lines.extend(indented_wrapped_lines(warning, 0, width, Theme::warning()));
        }
        if let Some(error) = self
            .computer_use
            .as_ref()
            .and_then(ComputerUseControl::terminal_error)
        {
            lines.push(Line::default());
            lines.push(heading_with_status("Connection error", "", width));
            lines.extend(indented_wrapped_lines(&error, 0, width, Theme::error()));
        }
        lines.push(Line::default());
        let driver = self
            .computer_use
            .as_ref()
            .and_then(ComputerUseControl::driver_path);
        lines.push(heading_with_status(
            "Cua Driver",
            if driver.is_some() {
                "Detected"
            } else {
                "Not detected"
            },
            width,
        ));
        let path = driver
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                "/computer setup installs the missing driver after separate consent".into()
            });
        lines.extend(indented_wrapped_lines(&path, 0, width, Theme::dim()));
        lines.extend(indented_wrapped_lines(
            "cua-driver doctor  installation diagnostics",
            0,
            width,
            Theme::dim(),
        ));
        if cfg!(target_os = "macos") {
            lines.extend(indented_wrapped_lines(
                "cua-driver permissions status  check permissions after the daemon starts",
                0,
                width,
                Theme::dim(),
            ));
        }
        lines.push(Line::default());
        lines.push(heading_with_status("Access and privacy", "", width));
        for note in [
            "Enabling grants local desktop access, including signed-in apps.",
            "No per-action Rho approval, including in supervised mode.",
            "Captured images go to your model provider and session history.",
            "This machine saves each session's access separately. Explicit on/off also sets the default for new sessions. Resumed sessions keep their own choice.",
        ] {
            lines.extend(indented_wrapped_lines(note, 0, width, Theme::text()));
        }
        lines.push(Line::default());
        lines.push(heading_with_status("Commands", "", width));
        for command in [
            next,
            "/computer setup  install, configure and verify connection",
        ] {
            lines.extend(indented_wrapped_lines(command, 0, width, Theme::text()));
        }
        lines
    }

    pub(super) fn computer_overlay_frame(&self, area: Rect) -> Option<OverlayPanelFrame> {
        let ComposerMode::Computer(overlay) = self.input_ui.composer() else {
            return None;
        };
        // Reserve the shared panel's scrollbar column before wrapping text.
        let lines = self.computer_status_lines(overlay_panel_inner_width(area).saturating_sub(1));
        Some(render_overlay_panel(
            "Computer use",
            if self.computer_installation_pending()
                || self.computer_use.as_ref().is_some_and(|control| {
                    matches!(
                        control.status(),
                        ComputerUseStatus::Connecting | ComputerUseStatus::Connected
                    )
                })
            {
                "r cancel setup / revoke access · Enter/Esc close"
            } else {
                "Enter/Esc close"
            },
            &lines,
            overlay.scroll.offset(),
            area,
        ))
    }

    pub(super) fn scroll_computer_overlay(
        &mut self,
        area: Rect,
        target: PanelScrollTarget,
    ) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::Computer(_)) {
            return false;
        }
        let body_len = self
            .computer_status_lines(overlay_panel_inner_width(area).saturating_sub(1))
            .len();
        let body_rows = overlay_panel_layout(area, body_len).body_rows;
        if let ComposerMode::Computer(overlay) = self.input_ui.composer_mut() {
            overlay.scroll.apply(target, body_len, body_rows);
        }
        true
    }

    pub(super) fn clamp_computer_overlay_scroll(&mut self, terminal: &ratatui::DefaultTerminal) {
        if let (ComposerMode::Computer(overlay), Ok(size)) =
            (self.input_ui.composer(), terminal.size())
        {
            let target = PanelScrollTarget::Absolute(overlay.scroll.offset());
            self.scroll_computer_overlay(Rect::new(0, 0, size.width, size.height), target);
        }
    }

    pub(super) fn handle_computer_overlay_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &ratatui::DefaultTerminal,
    ) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::Computer(_)) {
            return false;
        }
        if key.code == crossterm::event::KeyCode::Char('r') && key.modifiers.is_empty() {
            if self.computer_use.as_ref().is_some_and(|control| {
                control.installation_pending()
                    || matches!(
                        control.status(),
                        ComputerUseStatus::Connecting | ComputerUseStatus::Connected
                    )
            }) {
                self.revoke_computer_preference();
                self.show_computer_off();
            }
            return true;
        }
        match classify_panel_key(key) {
            PanelKey::Close => {
                self.input_ui.set_composer(ComposerMode::Input);
                true
            }
            PanelKey::Scroll(target) => {
                if let Ok(size) = terminal.size() {
                    self.scroll_computer_overlay(Rect::new(0, 0, size.width, size.height), target);
                }
                true
            }
            PanelKey::Passthrough => false,
            PanelKey::Swallow => true,
        }
    }
}
