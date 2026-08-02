use super::{App, InteractiveRuntime};
use crate::permission::PermissionMode;

impl App {
    pub(super) async fn apply_permission_mode(
        &mut self,
        mode: PermissionMode,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let previous = agent.permission_mode();
        agent.set_permission_mode(mode).await?;
        if let Err(error) = self.info.services.config_repository.update(|config| {
            config.permission_mode = mode;
        }) {
            if let Err(rollback_error) = agent.set_permission_mode(previous).await {
                return Err(anyhow::anyhow!(
                    "could not save permission mode: {error}; runtime rollback failed: {rollback_error}"
                ));
            }
            return Err(error);
        }
        self.info.runtime.permission_mode = mode;
        self.set_status(format!("permission mode: {}", mode.as_str()));
        Ok(())
    }

    pub(super) fn reject_permission_mode_change(&mut self) {
        self.set_status("permission mode cannot change until the current turn finishes");
    }
}
