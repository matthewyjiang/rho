use super::{agent_picker::InternalAgentModelPickerOrigin, App, ComposerMode};
use crate::{agent::PERMISSION_CLASSIFIER_AGENT_ID, permission::PermissionMode};

const SELECT_CLASSIFIER_MODEL_STARTUP_STATUS: &str =
    "select a permission classifier model for Auto mode";

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

impl App {
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
    pub(super) fn reconcile_auto_classifier_gate(&mut self) -> anyhow::Result<()> {
        let Some(demote) = self.auto_classifier_gate() else {
            return Ok(());
        };
        let mode = PermissionMode::Supervised;
        self.info
            .services
            .config_repository
            .update(|config| config.permission_mode = mode)?;
        self.configure_daemon(rho_protocol::SessionChange::PermissionMode { mode: mode.into() })?;
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
        if self.open_internal_agent_model_picker(
            PERMISSION_CLASSIFIER_AGENT_ID,
            InternalAgentModelPickerOrigin::PermissionModeStartup,
        ) {
            self.set_status(SELECT_CLASSIFIER_MODEL_STARTUP_STATUS);
            return None;
        }
        // Empty model catalog: Auto would fail closed on every gated tool.
        Some(AutoClassifierDemote::NoneAvailable)
    }

    /// Settles the startup classifier picker once a model was picked: Auto
    /// was already on, and setting it again makes the session read the
    /// model the picker saved.
    pub(super) fn finish_permission_classifier_startup_selection(
        &mut self,
        selected: bool,
    ) -> anyhow::Result<()> {
        // `selected == false` means model resolve failed after confirm, not Esc.
        // Cancel/demote lives only in cancel_permission_classifier_model_prompt.
        if !selected {
            return Ok(());
        }
        self.pending_auto_classifier_demote = false;
        self.configure_daemon(rho_protocol::SessionChange::PermissionMode {
            mode: PermissionMode::Auto.into(),
        })?;
        self.set_status("permission mode: auto");
        Ok(())
    }

    /// Dismisses a pending startup classifier prompt. It only marks a
    /// demote, which [`Self::reconcile_auto_classifier_gate`] applies on the
    /// next idle pass.
    pub(super) fn cancel_permission_classifier_model_prompt(
        &mut self,
        restore_input: bool,
    ) -> bool {
        if !self
            .internal_agent_model_target
            .as_ref()
            .is_some_and(|target| {
                target.id == PERMISSION_CLASSIFIER_AGENT_ID
                    && target.origin == InternalAgentModelPickerOrigin::PermissionModeStartup
            })
        {
            return false;
        }
        self.internal_agent_model_target = None;
        if restore_input {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        self.pending_auto_classifier_demote = true;
        self.set_status("permission mode set to supervised: no classifier model selected");
        true
    }
}

#[cfg(test)]
#[path = "permission_mode_tests.rs"]
mod tests;
