use super::{agent_picker::InternalAgentModelPickerOrigin, App, ComposerMode, InteractiveRuntime};
use crate::{agent::PERMISSION_CLASSIFIER_AGENT_ID, permission::PermissionMode};

const SELECT_CLASSIFIER_MODEL_STATUS: &str =
    "select a permission classifier model to turn Auto mode on";
const SELECT_CLASSIFIER_MODEL_STARTUP_STATUS: &str =
    "select a permission classifier model for Auto mode";

/// Whether a TUI mode change becomes the saved default for future launches.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PermissionPersistence {
    /// Write the mode to config, like `/permissions`.
    Save,
    /// Keep the mode for this process only, across `/new` and `/resume`.
    /// The next launch starts in the saved mode.
    UntilExit,
}

/// Why the Auto gate dropped a session to Supervised.
#[derive(Clone, Copy)]
enum AutoClassifierDemote {
    /// The person dismissed the classifier picker.
    NotSelected,
    /// No model could serve as the classifier.
    NoneAvailable,
}

impl AutoClassifierDemote {
    fn status(self) -> &'static str {
        match self {
            Self::NotSelected => "permission mode set to supervised: no classifier model selected",
            Self::NoneAvailable => {
                "permission mode set to supervised: no classifier model available; run /refresh-models"
            }
        }
    }
}

/// Cycle from most restrictive to least restrictive, then wrap back to Plan.
/// Auto is only reachable when the session has a classifier model configured.
fn next_cycle_mode(current: PermissionMode, classifier_configured: bool) -> PermissionMode {
    let mut modes = PermissionMode::ALL;
    modes.sort_by_key(|mode| mode.restrictiveness_rank());
    modes
        .iter()
        .copied()
        .filter(|mode| classifier_configured || *mode != PermissionMode::Auto)
        .find(|mode| mode.restrictiveness_rank() > current.restrictiveness_rank())
        .unwrap_or(modes[0])
}

impl App {
    pub(super) fn queue_permission_mode_cycle(&mut self) {
        if self.info.session.daemon.is_some() {
            let _ = self.cycle_daemon_permission();
            return;
        }
        let current = self
            .pending_permission_mode
            .unwrap_or(self.info.runtime.permission_mode);
        let next = next_cycle_mode(current, self.permission_classifier_model_configured());
        self.pending_permission_mode = Some(next);
        self.set_status(format!(
            "permission mode {} queued for next turn",
            next.as_str()
        ));
    }

    pub(super) async fn cycle_permission_mode(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.info.session.daemon.is_some() {
            return self.cycle_daemon_permission();
        }
        if agent.is_session_busy() {
            self.queue_permission_mode_cycle();
            return Ok(());
        }
        let next = next_cycle_mode(
            self.info.runtime.permission_mode,
            self.permission_classifier_model_configured(),
        );
        self.apply_permission_mode(next, PermissionPersistence::UntilExit, agent)
            .await
    }

    /// Apply queued changes only after the runtime releases the active run or
    /// compaction. Used at turn end and before idle follow-ups can start.
    pub(super) async fn apply_pending_permission_mode(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if agent.is_session_busy() {
            return Ok(false);
        }
        let Some(mode) = self.pending_permission_mode.take() else {
            return Ok(false);
        };
        self.apply_permission_mode(mode, PermissionPersistence::UntilExit, agent)
            .await?;
        Ok(true)
    }

    pub(super) async fn execute_permissions_command(
        &mut self,
        invocation: super::CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let args = invocation.args.trim();
        if args.is_empty() {
            self.insert_entry(&super::Entry::Notice(format!(
                "permission mode: {}. usage: /permissions [bypass|auto|allow_edits|plan|supervised]",
                agent.permission_mode().as_str()
            )));
            return Ok(());
        }
        let mode = match args.parse::<PermissionMode>() {
            Ok(mode) => mode,
            Err(error) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "could not change permission mode: {error}"
                )));
                return Ok(());
            }
        };
        if agent.is_session_busy() {
            if agent.is_compacting() {
                self.set_status("permission mode cannot change while compaction is active");
            } else {
                self.reject_permission_mode_change();
            }
            return Ok(());
        }
        if mode == PermissionMode::Auto && !self.permission_classifier_model_configured() {
            if !self.open_permission_classifier_model_prompt(
                InternalAgentModelPickerOrigin::PermissionModeCommand,
            ) {
                self.set_status(
                    "could not enable auto: no classifier model available; use /refresh-models",
                );
            }
            return Ok(());
        }
        if let Err(error) = self
            .apply_permission_mode(mode, PermissionPersistence::Save, agent)
            .await
        {
            self.insert_entry(&super::Entry::Error(format!(
                "could not change permission mode: {error}"
            )));
        }
        Ok(())
    }

    pub(super) async fn apply_permission_mode(
        &mut self,
        mode: PermissionMode,
        persistence: PermissionPersistence,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.info.session.daemon.is_some() {
            return self.apply_daemon_permission_mode(mode, persistence);
        }
        let previous = agent.permission_mode();
        let previous_config = agent.config_snapshot();
        let mut current_config = previous_config.clone();
        current_config
            .internal_agents
            .clone_from(&self.info.runtime.internal_agents);
        current_config.permission_mode = self.info.runtime.permission_mode;
        agent.update_config(current_config);
        if let Err(error) = agent.set_permission_mode(mode).await {
            agent.update_config(previous_config);
            return Err(error);
        }
        let save_result = match persistence {
            PermissionPersistence::Save => self.info.services.config_repository.update(|config| {
                config.permission_mode = mode;
            }),
            PermissionPersistence::UntilExit => Ok(()),
        };
        if let Err(error) = save_result {
            if let Err(rollback_error) = agent.set_permission_mode(previous).await {
                return Err(anyhow::anyhow!(
                    "could not save permission mode: {error}; runtime rollback failed: {rollback_error}"
                ));
            }
            agent.update_config(previous_config);
            return Err(error);
        }
        self.info.runtime.permission_mode = mode;
        let mut applied_config = agent.config_snapshot();
        applied_config.permission_mode = mode;
        agent.update_config(applied_config);
        self.set_status(format!("permission mode: {}", mode.as_str()));
        Ok(())
    }

    /// [`Self::apply_permission_mode`] for a daemon session, which applies
    /// the mode and reports it in its options.
    fn apply_daemon_permission_mode(
        &mut self,
        mode: PermissionMode,
        persistence: PermissionPersistence,
    ) -> anyhow::Result<()> {
        match persistence {
            PermissionPersistence::Save => self
                .info
                .services
                .config_repository
                .update(|config| config.permission_mode = mode)?,
            PermissionPersistence::UntilExit => {}
        }
        self.configure_daemon(rho_protocol::SessionChange::PermissionMode { mode: mode.into() })
    }

    pub(super) fn permission_classifier_model_configured(&self) -> bool {
        self.info
            .runtime
            .internal_agents
            .contains_key(PERMISSION_CLASSIFIER_AGENT_ID)
    }

    /// Keeps interactive Auto consistent with a configured classifier model.
    ///
    /// Call from idle transitions that land on a free composer (startup, resume,
    /// setup finish, full picker dismiss). Drains a pending startup-dismiss
    /// demote first, then opens the classifier picker when Auto is active
    /// without a model. No-op while setup or another picker owns the composer.
    pub(super) async fn reconcile_auto_classifier_gate(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(demote) = self.auto_classifier_gate() else {
            return Ok(());
        };
        self.apply_permission_mode(
            PermissionMode::Supervised,
            PermissionPersistence::Save,
            agent,
        )
        .await?;
        self.set_status(demote.status());
        Ok(())
    }

    /// [`Self::reconcile_auto_classifier_gate`] for a daemon session, which
    /// opens in Auto without a model too and needs no runtime handle to
    /// change its mode.
    pub(super) fn reconcile_daemon_auto_classifier_gate(&mut self) -> anyhow::Result<()> {
        let Some(demote) = self.auto_classifier_gate() else {
            return Ok(());
        };
        self.apply_daemon_permission_mode(PermissionMode::Supervised, PermissionPersistence::Save)?;
        self.set_status(demote.status());
        Ok(())
    }

    /// Opens the classifier picker when the gate needs it, or says why
    /// Auto must fall back to Supervised. Startup Auto without a classifier
    /// cannot run gated tools; Supervised keeps the same capability gate and
    /// asks a human instead.
    fn auto_classifier_gate(&mut self) -> Option<AutoClassifierDemote> {
        if self.pending_auto_classifier_demote {
            self.pending_auto_classifier_demote = false;
            return (self.info.runtime.permission_mode == PermissionMode::Auto
                && !self.permission_classifier_model_configured())
            .then_some(AutoClassifierDemote::NotSelected);
        }
        if self.exclusive.setup_step().is_some() {
            return None;
        }
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return None;
        }
        // A resumed daemon session's handoff dialog opens first when its
        // report answers, which runs the gate again.
        if self.awaited_daemon_report == Some(rho_protocol::SessionReport::ProviderContext) {
            return None;
        }
        if self.info.runtime.permission_mode != PermissionMode::Auto {
            return None;
        }
        if self.permission_classifier_model_configured() {
            return None;
        }
        if self.open_permission_classifier_model_prompt(
            InternalAgentModelPickerOrigin::PermissionModeStartup,
        ) {
            return None;
        }
        // Empty model catalog: Auto would fail closed on every gated tool.
        Some(AutoClassifierDemote::NoneAvailable)
    }

    pub(super) fn open_permission_classifier_model_prompt(
        &mut self,
        origin: InternalAgentModelPickerOrigin,
    ) -> bool {
        if !self.open_internal_agent_model_picker(PERMISSION_CLASSIFIER_AGENT_ID, origin) {
            return false;
        }
        let status = match origin {
            InternalAgentModelPickerOrigin::PermissionModeStartup => {
                SELECT_CLASSIFIER_MODEL_STARTUP_STATUS
            }
            InternalAgentModelPickerOrigin::AgentsPicker
            | InternalAgentModelPickerOrigin::PermissionModeCommand
            | InternalAgentModelPickerOrigin::PermissionScreenSetupCommand => {
                SELECT_CLASSIFIER_MODEL_STATUS
            }
        };
        self.set_status(status);
        true
    }

    pub(super) async fn finish_permission_classifier_model_selection(
        &mut self,
        selected: bool,
        origin: InternalAgentModelPickerOrigin,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        // `selected == false` means model resolve failed after confirm, not Esc.
        // Cancel/demote lives only in cancel_permission_classifier_model_prompt.
        if !selected {
            return Ok(());
        }
        match origin {
            InternalAgentModelPickerOrigin::PermissionModeCommand => {
                self.apply_permission_mode(
                    PermissionMode::Auto,
                    PermissionPersistence::Save,
                    agent,
                )
                .await?;
            }
            InternalAgentModelPickerOrigin::PermissionModeStartup => {
                // Auto is already active; the picker only stored the model.
                self.pending_auto_classifier_demote = false;
                if self.info.session.daemon.is_some() {
                    // Setting Auto again makes the daemon's session read the
                    // model the picker saved.
                    self.configure_daemon(rho_protocol::SessionChange::PermissionMode {
                        mode: PermissionMode::Auto.into(),
                    })?;
                }
                self.sync_internal_agents_runtime_config(agent);
                self.set_status("permission mode: auto");
            }
            _ => {}
        }
        Ok(())
    }

    /// Dismisses a pending classifier prompt opened to enable or repair Auto.
    /// Startup dismiss only marks a demote; [`Self::reconcile_auto_classifier_gate`]
    /// applies it once an idle path has a runtime handle.
    pub(super) fn cancel_permission_classifier_model_prompt(
        &mut self,
        restore_input: bool,
    ) -> bool {
        let origin = match self.internal_agent_model_target.as_ref() {
            Some(target)
                if target.id == PERMISSION_CLASSIFIER_AGENT_ID
                    && matches!(
                        target.origin,
                        InternalAgentModelPickerOrigin::PermissionModeCommand
                            | InternalAgentModelPickerOrigin::PermissionModeStartup
                    ) =>
            {
                target.origin
            }
            _ => return false,
        };
        self.internal_agent_model_target = None;
        if restore_input {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        match origin {
            InternalAgentModelPickerOrigin::PermissionModeCommand => {
                self.set_status(format!(
                    "permission mode stays {}: no classifier model selected",
                    self.info.runtime.permission_mode.as_str()
                ));
            }
            InternalAgentModelPickerOrigin::PermissionModeStartup => {
                self.pending_auto_classifier_demote = true;
                self.set_status("permission mode set to supervised: no classifier model selected");
            }
            _ => {}
        }
        true
    }

    /// Hands the runtime this session's `[internal_agents]` entries and
    /// permission mode, which the classifier and delegated catalog agents
    /// read from its config.
    pub(super) fn sync_internal_agents_runtime_config(&self, agent: &mut InteractiveRuntime) {
        let mut config = agent.config_snapshot();
        config
            .internal_agents
            .clone_from(&self.info.runtime.internal_agents);
        config.permission_mode = self.info.runtime.permission_mode;
        agent.update_config(config);
    }

    /// Hands the classifier settings saved by an editor that had no agent,
    /// such as the screen allow threshold. Runs on each idle pass and before
    /// each turn starts, so the next classification sees them.
    pub(super) fn apply_pending_classifier_config(&mut self, agent: &mut InteractiveRuntime) {
        if std::mem::take(&mut self.classifier_config_sync_pending) {
            self.sync_internal_agents_runtime_config(agent);
        }
    }

    pub(super) fn reject_permission_mode_change(&mut self) {
        self.set_status("permission mode cannot change until the current turn finishes");
    }
}

#[cfg(test)]
#[path = "permission_mode_tests.rs"]
mod tests;
