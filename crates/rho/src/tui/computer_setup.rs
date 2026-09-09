//! Setup chooses installation or connection, never combines their authorizations.

use super::{
    App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending, InteractiveRuntime,
};
use crate::tools::computer_use::{
    setup_platform, ComputerSetupUpdate, ComputerUseControl, ComputerUseStatus,
    INSTALLATION_RECOVERY,
};

impl App {
    pub(in crate::tui) fn computer_installation_pending(&self) -> bool {
        self.computer_use
            .as_ref()
            .is_some_and(ComputerUseControl::installation_pending)
    }

    pub(super) fn setup_computer(&mut self, agent: &InteractiveRuntime) -> anyhow::Result<()> {
        if !self.can_grant_computer_access(agent) {
            return Ok(());
        }
        let Some(session) = agent.computer_use() else {
            self.insert_entry(&Entry::Error(
                "computer setup requires an interactive native session with tools enabled".into(),
            ));
            return Ok(());
        };
        if self.computer_installation_pending() {
            self.set_status("Cua Driver installation pending; /computer off cancels");
            return Ok(());
        }
        if session.status() != ComputerUseStatus::Off {
            self.show_computer_status();
            return Ok(());
        }
        if let Some(path) = session.driver_path() {
            self.insert_entry(&Entry::Notice(format!("Cua Driver configured for this session: {}. Installation skipped; no persistent MCP config or access grant written", path.display())));
            return self.prompt_computer_access();
        }
        let platform = setup_platform();
        let (source, locations, notes) = (platform.source, platform.locations, platform.notes);
        self.input_ui.set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
            choice: InlineChoice::new(
                "Install Cua Driver?",
                format!("Download and execute {source}. Writes Cua installation files to {locations}. Rho disables Cua telemetry before installation and saves the opt-out afterward if setup completes. No PATH/profile edits requested. Installation may replace Cua files and stop old Cua daemons. {notes} This does not grant Rho desktop access; that requires a separate confirmation. Cancelling cannot undo files already written or guarantee the saved telemetry opt-out."),
                vec![
                    InlineChoiceOption::available("cancel", 'c', "Cancel", "Do not install"),
                    InlineChoiceOption::available("install", 'i', "Install driver", "Download and execute Cua's installer"),
                ],
            )?,
            pending: InlineChoicePending::ComputerInstall,
            parent_picker: None,
        }));
        Ok(())
    }

    pub(in crate::tui) fn confirm_computer_installation(
        &mut self,
        value: &str,
        agent: &InteractiveRuntime,
    ) {
        if value != "install" {
            self.set_status("Cua Driver installation not authorized");
            return;
        }
        if !self.can_grant_computer_access(agent) {
            return;
        }
        match agent.install_computer_driver() {
            Ok(log) => {
                self.insert_entry(&Entry::Notice(format!("installing Cua Driver; desktop access remains off. /computer off cancels. Installer output: {}", log.display())));
            }
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not install Cua Driver: {error}"
            ))),
        }
    }

    pub(super) fn poll_computer_installation(&mut self, agent: &InteractiveRuntime) -> bool {
        let Some(result) = agent
            .computer_use()
            .and_then(|session| session.take_installation_result())
        else {
            return false;
        };
        match result {
            ComputerSetupUpdate::Installed => {
                self.insert_entry(&Entry::Notice("Cua Driver installed and detected; configured for this session without a persistent grant. Desktop access is still off; /computer setup continues to access confirmation and connection verification".into()));
                // Do not replace another overlay or discard a prompt being typed.
                if matches!(self.input_ui.composer(), ComposerMode::Input)
                    && self.input_ui.text().is_empty()
                    && self.can_grant_computer_access(agent)
                {
                    if let Err(error) = self.prompt_computer_access() {
                        self.insert_entry(&Entry::Error(format!(
                            "could not show desktop access confirmation: {error}"
                        )));
                    }
                }
            }
            ComputerSetupUpdate::Cancelled => self.insert_entry(&Entry::Notice(format!("Cua Driver installation cancelled; desktop access was not granted. {INSTALLATION_RECOVERY}"))),
            ComputerSetupUpdate::Failed(error) => self.insert_entry(&Entry::Error(format!(
                "could not complete Cua Driver setup: {error}"
            ))),
        }
        true
    }
}
