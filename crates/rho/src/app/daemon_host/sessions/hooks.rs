//! Reloads a loaded session's hooks from their files between its turns
//! (`rho/session/hooks/reload`), as the TUI's `/hooks` asks. Reading the
//! session's hooks report never does.

use rho_protocol::{
    HooksReloadOutcome, RpcError, SessionHooksReloadParams, SessionHooksReloadResponse,
};

use super::{inspect::not_loaded, Sessions};

impl Sessions {
    /// Reloads the session's hooks. Files that do not load leave its hooks
    /// as they were, which the answer says, rather than failing the request.
    pub(in crate::app::daemon_host) async fn reload_hooks(
        &self,
        params: SessionHooksReloadParams,
    ) -> Result<SessionHooksReloadResponse, RpcError> {
        let session = self.between_turns(&params.session).await?;
        let runtime = session.runtime.lock().await;
        let runtime = runtime.as_ref().ok_or_else(not_loaded)?;
        let outcome = match runtime.reload_hooks() {
            Ok(()) => HooksReloadOutcome::Reloaded,
            Err(error) => HooksReloadOutcome::Refused {
                reason: error.to_string(),
            },
        };
        Ok(SessionHooksReloadResponse::new(outcome))
    }
}
