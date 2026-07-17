use super::{App, Entry, InteractiveRuntime};
use crate::permission::PermissionMode;

impl App {
    pub(super) async fn apply_permission_mode(
        &mut self,
        mode: PermissionMode,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let previous = agent.permission_mode();
        agent.set_permission_mode(mode).await?;
        if let Err(error) = self.info.config_repository.update(|config| {
            config.permission_mode = mode;
        }) {
            if let Err(rollback_error) = agent.set_permission_mode(previous).await {
                return Err(anyhow::anyhow!(
                    "could not save permission mode: {error}; runtime rollback failed: {rollback_error}"
                ));
            }
            return Err(error);
        }
        self.info.permission_mode = mode;
        let notice = format!("permission mode: {}", mode.as_str());
        self.insert_entry(&Entry::Notice(notice.clone()));
        self.status = notice;
        Ok(())
    }

    pub(super) fn reject_permission_mode_change(&mut self) {
        self.insert_entry(&Entry::Notice(
            "permission mode cannot change until the current turn finishes".into(),
        ));
        self.status = "permission mode unavailable while running".into();
    }
}
