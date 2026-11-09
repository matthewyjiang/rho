//! Read the loaded session's shared MCP inventory without locking its runtime.

use std::sync::PoisonError;

use rho_protocol::{ErrorCode, RpcError, SessionMcpInventory, SessionMcpParams};

use super::Sessions;

impl Sessions {
    pub(in crate::app::daemon_host) async fn mcp(
        &self,
        params: SessionMcpParams,
    ) -> Result<SessionMcpInventory, RpcError> {
        self.refuse_when_stopping()?;
        let not_loaded = || {
            RpcError::new(
                ErrorCode::SessionNotLoaded,
                "the daemon has not loaded the session; attach to it first",
            )
        };
        let session = self
            .loaded_session(&params.session)
            .await?
            .ok_or_else(not_loaded)?;
        // An idle unload may have taken it since the lookup. Reading an
        // inventory needs no residency hold, since teardown does not change it.
        let turn = session.enter().ok_or_else(not_loaded)?;
        let inventory = session
            .mcp
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        drop(turn);
        Ok(inventory.snapshot())
    }
}
