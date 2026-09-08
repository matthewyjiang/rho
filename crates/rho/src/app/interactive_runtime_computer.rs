//! Session-scoped desktop authorization. Never persisted into config or snapshots.

use crate::{
    permission::PermissionMode,
    tools::computer_use::{ComputerUseSession, ComputerUseStatus},
};

use super::InteractiveRuntime;

impl InteractiveRuntime {
    pub(crate) fn computer_use(&self) -> Option<&ComputerUseSession> {
        self.tools.computer_use()
    }

    pub(crate) fn authorize_computer_use(&self) -> anyhow::Result<ComputerUseSession> {
        if self.is_session_busy() {
            anyhow::bail!("computer use can only be enabled while the session is idle");
        }
        if self.permission_mode == PermissionMode::Plan {
            anyhow::bail!("computer use is unavailable in plan mode");
        }
        self.tools.computer_use().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "computer use requires an interactive native session with tools enabled"
            )
        })
    }

    /// Commit a completed connection to the next model turn. Failure revokes
    /// access instead of leaving an unadvertised desktop session running.
    pub(crate) async fn enable_connected_computer_use(&mut self) -> anyhow::Result<()> {
        let session = self.authorize_computer_use()?;
        if session.status() != ComputerUseStatus::Connected {
            anyhow::bail!("computer use is not connected");
        }
        if !self.tools.set_computer_use_registered(true) {
            return Ok(());
        }
        if let Err(error) = self.rebind_current_session().await {
            self.tools.set_computer_use_registered(false);
            session.disconnect().await;
            return Err(error);
        }
        self.remember_tool_list();
        Ok(())
    }

    /// Revocation takes effect before rebuilding the model's tool list. If the
    /// rebuild fails, any old registered tool still fails closed.
    pub(crate) async fn disable_computer_use(&mut self) -> anyhow::Result<()> {
        if let Some(session) = self.tools.computer_use() {
            session.disconnect().await;
        }
        if self.tools.set_computer_use_registered(false) {
            self.rebind_current_session().await?;
            self.remember_tool_list();
        }
        Ok(())
    }
}
