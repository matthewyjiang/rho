//! Session-scoped desktop authorization. Never persisted into config or snapshots.

use crate::{
    permission::PermissionMode,
    tools::computer_use::{ComputerUseSession, ComputerUseStatus},
};

use super::InteractiveRuntime;

#[cfg(test)]
#[path = "interactive_runtime_computer_tests.rs"]
mod tests;

pub(crate) enum ComputerUseUpdate {
    Unchanged,
    Connected,
    ConnectionFailed(String),
    Revoked(String),
}

impl InteractiveRuntime {
    pub(crate) fn computer_use(&self) -> Option<&ComputerUseSession> {
        self.tools.computer_use()
    }

    fn authorize_computer_use(&self) -> anyhow::Result<ComputerUseSession> {
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

    pub(crate) fn enable_computer_use(&self) -> anyhow::Result<()> {
        self.authorize_computer_use()?.start_connect()
    }

    /// The idle boundary owns registration for both activation and revocation,
    /// including revocations from retained tools or the during-turn UI handle.
    /// Driver failures are updates, not runtime failures. Only registration or
    /// session rebind errors may prevent the next turn from starting.
    pub(crate) async fn reconcile_computer_use(&mut self) -> anyhow::Result<ComputerUseUpdate> {
        if self.is_session_busy() {
            return Ok(ComputerUseUpdate::Unchanged);
        }
        let Some(session) = self.computer_use().cloned() else {
            return Ok(ComputerUseUpdate::Unchanged);
        };
        let connection_result = session.take_connect_result().await;
        session.finish_closing(/*wait*/ false).await;
        let desired = session.status() == ComputerUseStatus::Connected;
        if self.tools.set_computer_use_registered(desired) {
            if let Err(error) = self.rebind_current_session().await {
                // Restore the registry so the next boundary retries the bind.
                // Any retained tool still fails closed after revocation.
                self.tools.set_computer_use_registered(!desired);
                session.revoke();
                return Err(error);
            }
            self.remember_tool_list();
        }
        Ok(match connection_result {
            Some(Ok(())) if desired => ComputerUseUpdate::Connected,
            Some(Err(error)) => ComputerUseUpdate::ConnectionFailed(error.to_string()),
            Some(Ok(())) | None => session
                .take_revocation_notice()
                .map_or(ComputerUseUpdate::Unchanged, ComputerUseUpdate::Revoked),
        })
    }

    pub(crate) async fn disable_computer_use(&mut self) -> anyhow::Result<()> {
        if let Some(session) = self.tools.computer_use() {
            session.revoke();
        }
        match self.reconcile_computer_use().await? {
            ComputerUseUpdate::Revoked(notice) => self.sessions.queue_notice(notice),
            ComputerUseUpdate::Unchanged
            | ComputerUseUpdate::Connected
            | ComputerUseUpdate::ConnectionFailed(_) => {}
        }
        Ok(())
    }
}
