//! Explicit desktop access, kept separate from ordinary MCP configuration.

use crate::app::interactive_runtime::ComputerUseUpdate;
use crate::tools::computer_use::{desktop_warning, ComputerUseSession, ComputerUseStatus};

use super::{
    App, CommandInvocation, ComposerMode, Entry, InlineChoice, InlineChoiceModal,
    InlineChoiceOption, InlineChoicePending, InteractiveRuntime,
};

#[path = "computer_setup.rs"]
mod setup;

impl App {
    pub(super) async fn execute_computer_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.computer_use = agent.computer_use().map(ComputerUseSession::control);
        match invocation.args.trim() {
            "setup" => self.setup_computer(agent)?,
            "on" => {
                if !self.can_grant_computer_access(agent) {
                    return Ok(());
                }
                if self.computer_connect_pending() {
                    self.set_status("computer use is connecting; /computer off cancels");
                    return Ok(());
                }
                if self.computer_installation_pending() {
                    self.set_status("Cua Driver installation pending; /computer off cancels");
                    return Ok(());
                }
                if self.computer_use.as_ref().is_some_and(|control| {
                    matches!(
                        control.status(),
                        ComputerUseStatus::Connected | ComputerUseStatus::Closing
                    )
                }) {
                    self.show_computer_status();
                    return Ok(());
                }
                self.prompt_computer_access()?;
            }
            "off" | "stop" => {
                if let Err(error) = agent.disable_computer_use().await {
                    self.insert_entry(&Entry::Error(format!(
                        "could not refresh computer tool registration: {error}"
                    )));
                }
                self.show_computer_off();
            }
            _ => self.show_computer_command(&invocation),
        }
        Ok(())
    }

    fn prompt_computer_access(&mut self) -> anyhow::Result<()> {
        let choice = InlineChoice::new(
            "Grant desktop access?",
            "Rho can control your local desktop, including signed-in apps. No per-action Rho approval, even in supervised mode. Captured images go to your model provider and session history.",
            vec![
                InlineChoiceOption::available("cancel", 'c', "Cancel", "Keep access off"),
                InlineChoiceOption::available("grant", 'g', "Grant access", "This session only; /computer off revokes"),
            ],
        )?;
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ComputerAccess,
                parent_picker: None,
            }));
        Ok(())
    }

    pub(super) async fn execute_computer_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        match invocation.args.trim() {
            "off" | "stop" => {
                // Revoke the shared handle immediately. The old registered tool
                // fails closed; the runtime can refresh its registry when idle.
                if let Some(session) = &self.computer_use {
                    session.revoke();
                }
                self.show_computer_off();
            }
            "on" | "setup" => self.set_status(
                "interrupt the current turn before setting up or enabling computer use",
            ),
            _ => self.show_computer_command(&invocation),
        }
        Ok(())
    }

    fn show_computer_command(&mut self, invocation: &CommandInvocation) {
        match invocation.args.trim() {
            "" | "status" => self.show_computer_status(),
            _ => self.insert_entry(&Entry::Error(
                "usage: /computer [status|setup|on|off]".into(),
            )),
        }
    }

    pub(super) fn show_computer_off(&mut self) {
        if self.computer_installation_pending() {
            self.insert_entry(&Entry::Notice("Cua Driver installation cancellation requested; partial files may remain. Desktop access not granted".into()));
            self.set_status("computer setup cancellation requested; desktop access off");
            return;
        }
        self.insert_entry(&Entry::Notice("computer use off; access revoked while the transport closes. Completed desktop actions cannot be undone by disconnecting".into()));
        self.set_status("computer use off");
    }

    fn can_grant_computer_access(&mut self, agent: &InteractiveRuntime) -> bool {
        if agent.is_session_busy() {
            self.set_status("interrupt the current turn before granting computer access");
            return false;
        }
        if self.info.runtime.permission_mode == crate::permission::PermissionMode::Plan {
            self.insert_entry(&Entry::Error(
                "computer use is unavailable in plan mode".into(),
            ));
            return false;
        }
        true
    }

    pub(super) fn confirm_computer_access(&mut self, value: &str, agent: &mut InteractiveRuntime) {
        if value != "grant" {
            self.set_status("computer access not granted");
            return;
        }
        if !self.can_grant_computer_access(agent) {
            return;
        }
        if let Err(error) = agent.enable_computer_use() {
            self.insert_entry(&Entry::Error(format!(
                "could not grant computer access: {error}"
            )));
            return;
        }
        self.insert_entry(&Entry::Notice(
            "computer access granted; connecting to Cua Driver. /computer off revokes access"
                .into(),
        ));
        if let Some(warning) = desktop_warning() {
            self.insert_entry(&Entry::Notice(format!("warning: {warning}")));
        }
    }

    /// A shortcut must not grant access while the terminal clips the disclosure.
    pub(super) fn computer_consent_visible(&mut self, terminal: &ratatui::DefaultTerminal) -> bool {
        let Ok(size) = terminal.size() else {
            self.set_status(
                "could not check consent visibility; resize the terminal or Esc cancel",
            );
            return false;
        };
        let frame = self.frame_context(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        let needed = frame.composer.lines.len();
        let visible = usize::from(frame.layout.composer.height);
        if frame.layout.composer_start != 0 || visible < needed {
            self.set_status(format!("enlarge terminal to review computer consent: consent needs {needed} rows, {visible} visible; Esc cancel"));
            return false;
        }
        true
    }

    pub(super) fn computer_connect_pending(&self) -> bool {
        self.computer_use
            .as_ref()
            .is_some_and(|control| control.status() == ComputerUseStatus::Connecting)
    }

    pub(super) fn computer_lifecycle_pending(&self) -> bool {
        self.computer_use
            .as_ref()
            .is_some_and(|control| match control.status() {
                ComputerUseStatus::Installing
                | ComputerUseStatus::Connecting
                | ComputerUseStatus::Closing => true,
                ComputerUseStatus::Off | ComputerUseStatus::Connected => false,
            })
    }

    pub(super) async fn poll_computer_connection(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> bool {
        if agent.is_session_busy() {
            return false;
        }
        let setup_changed = self.poll_computer_installation(agent);
        match agent.reconcile_computer_use().await {
            Ok(ComputerUseUpdate::Unchanged) => return setup_changed,
            Ok(ComputerUseUpdate::Connected) => {
                self.insert_entry(&Entry::Notice("computer use connected through Cua Driver; the computer tool is available for the next turn".into()));
                self.set_status("computer use connected");
                self.insert_entry(&Entry::Notice("driver handshake verified; OS desktop capture and input permissions are not checked. Use cua-driver doctor for diagnostics; on macOS, use cua-driver permissions status after the daemon starts".into()));
            }
            Ok(ComputerUseUpdate::ConnectionFailed(error)) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not connect computer use: {error}"
                )));
                self.set_status("computer use off; /computer setup for help");
            }
            Ok(ComputerUseUpdate::Revoked(reason)) => {
                self.insert_entry(&Entry::Notice(reason));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not refresh computer tool registration: {error}"
                )));
            }
        }
        true
    }
}
