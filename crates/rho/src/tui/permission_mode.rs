use super::{App, InteractiveRuntime};
use crate::permission::PermissionMode;

impl App {
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

    pub(super) fn reject_permission_mode_change(&mut self) {
        self.set_status("permission mode cannot change until the current turn finishes");
    }
}
