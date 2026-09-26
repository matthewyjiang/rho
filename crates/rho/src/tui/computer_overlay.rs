//! Live computer dashboard with immediate revocation, never implicit grants.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;

use super::{
    overlay_panel::{PanelBody, PanelKeyOutcome, PanelState},
    panel_text::{heading_with_status, indented_wrapped_lines},
    theme::Theme,
    App, ComposerMode, PanelOverlay,
};
use crate::tools::computer_use::{desktop_warning, ComputerUseControl, ComputerUseStatus};

const TITLE: &str = "Computer use";
const FOOTER_REVOCABLE: &str = "r cancel setup / revoke access · Enter/Esc close";
const FOOTER: &str = "Enter/Esc close";

pub(super) struct ComputerOverlay {
    /// The app's control handle, re-synced by the connection poll so status
    /// stays live while the panel is open.
    control: Option<ComputerUseControl>,
    panel: PanelState,
}

impl std::fmt::Debug for ComputerOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputerOverlay")
            .field("status", &self.status())
            .field("panel", &self.panel)
            .finish()
    }
}

impl ComputerOverlay {
    fn status(&self) -> ComputerUseStatus {
        self.control
            .as_ref()
            .map(ComputerUseControl::status)
            .unwrap_or(ComputerUseStatus::Off)
    }

    /// `r` has something to cancel or revoke.
    fn revocable(&self) -> bool {
        self.control.as_ref().is_some_and(|control| {
            control.installation_pending()
                || matches!(
                    control.status(),
                    ComputerUseStatus::Connecting | ComputerUseStatus::Connected
                )
        })
    }

    fn status_lines(&self, width: usize) -> Vec<Line<'static>> {
        let state = self
            .control
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
            .control
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
            .control
            .as_ref()
            .and_then(ComputerUseControl::terminal_error)
        {
            lines.push(Line::default());
            lines.push(heading_with_status("Connection error", "", width));
            lines.extend(indented_wrapped_lines(&error, 0, width, Theme::error()));
        }
        lines.push(Line::default());
        let driver = self
            .control
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
}

impl PanelBody for ComputerOverlay {
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
        if self.revocable() {
            FOOTER_REVOCABLE
        } else {
            FOOTER
        }
    }

    fn body_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        self.status_lines(width)
    }

    fn handle_key(&mut self, key: KeyEvent) -> PanelKeyOutcome {
        if key.code != KeyCode::Char('r') || !key.modifiers.is_empty() {
            return PanelKeyOutcome::Unhandled;
        }
        if !self.revocable() {
            return PanelKeyOutcome::Handled;
        }
        PanelKeyOutcome::Run(|app| {
            app.revoke_computer_preference();
            app.show_computer_off();
        })
    }
}

impl App {
    pub(super) fn show_computer_status(&mut self) {
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Computer(
                ComputerOverlay {
                    control: self.computer_use.clone(),
                    panel: PanelState::default(),
                },
            )));
    }

    /// Point an open dashboard at the app's current control handle.
    pub(super) fn sync_computer_overlay(&mut self) {
        let control = self.computer_use.clone();
        if let ComposerMode::Panel(PanelOverlay::Computer(overlay)) = self.input_ui.composer_mut() {
            overlay.control = control;
        }
    }
}
