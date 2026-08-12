use super::{agent_picker::InternalAgentModelPickerOrigin, App, ComposerMode, InteractiveRuntime};
use crate::{
    agent::{effective_internal_agent_reasoning, PERMISSION_CLASSIFIER_AGENT_ID},
    permission::PermissionMode,
};

const SELECT_CLASSIFIER_MODEL_STATUS: &str =
    "select a permission classifier model to turn Auto mode on";
const SELECT_CLASSIFIER_MODEL_EDIT_STATUS: &str = "select a permission classifier model";

impl App {
    pub(super) async fn select_permission_mode_from_config(
        &mut self,
        mode: PermissionMode,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if mode == PermissionMode::Auto && !self.permission_classifier_model_configured() {
            self.open_permission_classifier_model_prompt(
                InternalAgentModelPickerOrigin::PermissionModeConfigRow,
            );
            return Ok(());
        }
        self.apply_permission_mode(mode, agent).await?;
        self.open_main_config_picker_selected(super::config_picker::PERMISSION_MODE_VALUE)
    }

    pub(super) async fn apply_permission_mode(
        &mut self,
        mode: PermissionMode,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let previous = agent.permission_mode();
        let previous_config = agent.config_snapshot();
        let mut current_config = previous_config.clone();
        current_config
            .internal_agents
            .clone_from(&self.info.runtime.internal_agents);
        current_config.permission_mode = self.info.runtime.permission_mode;
        agent.update_config(current_config);
        agent.set_permission_mode(mode).await?;
        if let Err(error) = self.info.services.config_repository.update(|config| {
            config.permission_mode = mode;
        }) {
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

    pub(super) fn permission_classifier_model_configured(&self) -> bool {
        self.info
            .runtime
            .internal_agents
            .contains_key(PERMISSION_CLASSIFIER_AGENT_ID)
    }

    pub(super) fn open_permission_classifier_model_prompt(
        &mut self,
        origin: InternalAgentModelPickerOrigin,
    ) {
        if self.open_internal_agent_model_picker(PERMISSION_CLASSIFIER_AGENT_ID, origin) {
            let status = match origin {
                InternalAgentModelPickerOrigin::PermissionClassifierModelConfigRow => {
                    SELECT_CLASSIFIER_MODEL_EDIT_STATUS
                }
                InternalAgentModelPickerOrigin::AgentsPicker
                | InternalAgentModelPickerOrigin::AdvisorCommand
                | InternalAgentModelPickerOrigin::AdvisorConfigRow
                | InternalAgentModelPickerOrigin::AdvisorModelConfigRow
                | InternalAgentModelPickerOrigin::PermissionModeConfigRow => {
                    SELECT_CLASSIFIER_MODEL_STATUS
                }
            };
            self.set_status(status);
        }
    }

    pub(super) async fn finish_permission_classifier_model_selection(
        &mut self,
        selected: bool,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if selected {
            self.apply_permission_mode(PermissionMode::Auto, agent)
                .await?;
        }
        Ok(())
    }

    pub(super) fn cancel_permission_classifier_model_prompt(
        &mut self,
        restore_input: bool,
    ) -> bool {
        let pending = matches!(
            self.internal_agent_model_target.as_ref(),
            Some(target)
                if target.id == PERMISSION_CLASSIFIER_AGENT_ID
                    && target.origin == InternalAgentModelPickerOrigin::PermissionModeConfigRow
        );
        if pending {
            self.internal_agent_model_target = None;
            if restore_input {
                self.input_ui.set_composer(ComposerMode::Input);
            }
            self.set_status(format!(
                "permission mode stays {}: no classifier model selected",
                self.info.runtime.permission_mode.as_str()
            ));
        }
        pending
    }

    pub(super) fn cycle_permission_classifier_reasoning(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(mut selection) = self
            .info
            .runtime
            .internal_agents
            .get(PERMISSION_CLASSIFIER_AGENT_ID)
            .cloned()
        else {
            self.set_status("select a permission classifier model first");
            return Ok(());
        };
        let capabilities = crate::agent::internal_agent_reasoning_capabilities(&selection);
        if capabilities == rho_providers::model::ReasoningCapabilities::NotConfigurable {
            return Ok(());
        }
        let current =
            effective_internal_agent_reasoning(PERMISSION_CLASSIFIER_AGENT_ID, &selection);
        let reasoning = capabilities.next_level(current);
        selection.reasoning = Some(reasoning);
        self.info
            .runtime
            .internal_agents
            .insert(PERMISSION_CLASSIFIER_AGENT_ID.into(), selection.clone());
        match self.info.services.config_repository.update(|config| {
            config.set_internal_agent_model_config(PERMISSION_CLASSIFIER_AGENT_ID, selection);
        }) {
            Ok(()) => {
                self.set_status(format!("permission classifier reasoning: {reasoning}"));
            }
            Err(err) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "permission classifier reasoning set to {reasoning} for this session, but saving config failed: {err}"
                )));
                self.set_status("config save failed");
            }
        }
        if self.info.runtime.permission_mode == PermissionMode::Auto {
            self.sync_permission_classifier_runtime_config(agent);
        }
        Ok(())
    }

    pub(super) fn sync_permission_classifier_runtime_config(&self, agent: &mut InteractiveRuntime) {
        let mut config = agent.config_snapshot();
        config
            .internal_agents
            .clone_from(&self.info.runtime.internal_agents);
        config.permission_mode = self.info.runtime.permission_mode;
        agent.update_config(config);
    }

    pub(super) fn reject_permission_mode_change(&mut self) {
        self.set_status("permission mode cannot change until the current turn finishes");
    }
}
