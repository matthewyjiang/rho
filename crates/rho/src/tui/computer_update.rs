//! Driver update checks and consent. A check only runs the trusted driver, so
//! it is allowed in any mode; installing needs an idle, non-plan session with
//! desktop access off, and its own cancel-default confirmation.

use super::{
    App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending, InteractiveRuntime,
};
use crate::tools::computer_use::{
    setup_platform, ComputerUseControl, ComputerUsePreference, ComputerUseStatus,
    UpdateCheckStatus, UpdateOutcome,
};

/// Whether a request may continue from a finished check to install consent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateRequest {
    /// Idle and eligible: may offer the install confirmation.
    OfferInstall,
    /// Busy turn or plan mode: check and report only.
    CheckOnly,
}

impl App {
    /// Start a driver update check only when the driver is already trusted to
    /// run: access is live, or this session's saved choice is on. Otherwise the
    /// dashboard offers an explicit check, since checking executes the driver.
    pub(super) fn auto_check_computer_update(&self) {
        let Some(control) = &self.computer_use else {
            return;
        };
        if control.update_check_status() != UpdateCheckStatus::NotChecked {
            return;
        }
        let trusted = match control.status() {
            ComputerUseStatus::Connecting | ComputerUseStatus::Connected => true,
            ComputerUseStatus::Installing | ComputerUseStatus::Closing => false,
            ComputerUseStatus::Off => {
                control.saved_session_preference() == Some(ComputerUsePreference::Enabled)
            }
        };
        if trusted {
            // A refusal (installer running) just leaves the dashboard unchecked.
            let _ = control.start_update_check();
        }
    }

    /// Dashboard `u`: explicitly (re)check. Never opens install consent, so it
    /// is safe during a turn or in plan mode.
    pub(in crate::tui) fn check_computer_update(&mut self) {
        let Some(control) = self.computer_use.clone() else {
            return;
        };
        match control.start_update_check() {
            Ok(()) => self.set_status("checking for Cua Driver updates"),
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not check for Cua Driver updates: {error}"
            ))),
        }
    }

    /// `/computer update`: check if needed, then ask to install the release
    /// the check reported when `request` allows it.
    pub(super) fn update_computer(&mut self, request: UpdateRequest) {
        let Some(control) = self.computer_use.clone() else {
            return;
        };
        let (current, latest) = match control.update_check_status() {
            UpdateCheckStatus::Checking => {
                self.set_status("checking for Cua Driver updates");
                self.show_computer_status();
                return;
            }
            UpdateCheckStatus::Checked(UpdateOutcome::Available {
                current, latest, ..
            }) => (current, latest),
            UpdateCheckStatus::Checked(
                UpdateOutcome::UpToDate { .. } | UpdateOutcome::Unavailable { .. },
            )
            | UpdateCheckStatus::NotChecked
            | UpdateCheckStatus::Failed(_) => {
                self.check_computer_update();
                self.show_computer_status();
                return;
            }
        };
        let blocker = match (request, control.status()) {
            (UpdateRequest::CheckOnly, _) => {
                Some("finish the current turn and leave plan mode first")
            }
            (UpdateRequest::OfferInstall, ComputerUseStatus::Off) => None,
            (UpdateRequest::OfferInstall, ComputerUseStatus::Installing) => {
                Some("an installation is already pending")
            }
            (
                UpdateRequest::OfferInstall,
                ComputerUseStatus::Connecting
                | ComputerUseStatus::Connected
                | ComputerUseStatus::Closing,
            ) => Some("/computer off first; updating never reconnects automatically"),
        };
        if let Some(blocker) = blocker {
            self.insert_entry(&Entry::Error(format!(
                "could not update Cua Driver to {latest}: {blocker}"
            )));
            return;
        }
        if let Err(error) = control.ensure_managed_driver() {
            self.insert_entry(&Entry::Error(format!(
                "could not update Cua Driver: {error}"
            )));
            return;
        }
        let platform = setup_platform();
        let choice = InlineChoice::new(
            format!("Update Cua Driver {current} to {latest}?"),
            format!("Download and execute {}, pinned to {latest}. Writes to {}. The installer may replace Cua files and stop running Cua daemons. {} Rho forces Cua telemetry off for the installer and leaves your saved telemetry preference unchanged. Desktop access stays off; /computer on reconnects and saves access on again.", platform.source, platform.locations, platform.notes),
            vec![
                InlineChoiceOption::available("cancel", 'c', "Cancel", "Keep the current driver"),
                InlineChoiceOption::available("update", 'u', "Update driver", "Download and execute Cua's installer").require_full_visibility(),
            ],
        );
        match choice {
            Ok(choice) => {
                self.input_ui
                    .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                        choice,
                        pending: InlineChoicePending::ComputerUpdate {
                            from: current,
                            to: latest,
                        },
                        parent_picker: None,
                    }))
            }
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not show Cua Driver update confirmation: {error}"
            ))),
        }
    }

    pub(in crate::tui) fn confirm_computer_update(
        &mut self,
        value: &str,
        from: &str,
        to: &str,
        agent: &InteractiveRuntime,
    ) {
        if value != "update" {
            self.set_status("Cua Driver update not authorized");
            return;
        }
        match agent.update_computer_driver(from, to) {
            Ok(log) => self.insert_entry(&Entry::Notice(format!(
                "updating Cua Driver to {to}; desktop access remains off. /computer off cancels. Installer output: {}",
                log.display()
            ))),
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not update Cua Driver: {error}"
            ))),
        }
    }
}

impl ComputerUseControl {
    /// Short dashboard hint for what the user can do about an available update.
    pub(in crate::tui) fn update_hint(&self) -> &'static str {
        match self.status() {
            ComputerUseStatus::Off => "/computer update installs it",
            ComputerUseStatus::Installing => "installation pending",
            ComputerUseStatus::Connecting
            | ComputerUseStatus::Connected
            | ComputerUseStatus::Closing => "/computer off, then /computer update",
        }
    }
}
