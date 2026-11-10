//! Reads a report about a loaded session between its turns.

use rho_protocol::{
    ErrorCode, RpcError, SessionInspectParams, SessionInspectResponse, SessionReport,
};

use super::Sessions;

impl Sessions {
    pub(in crate::app::daemon_host) async fn inspect(
        &self,
        params: SessionInspectParams,
    ) -> Result<SessionInspectResponse, RpcError> {
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
        // A report reads what a turn or a change is changing, and a client
        // asked to see it now: refused, not queued behind them.
        let runtime = session.runtime.try_lock().map_err(|_| {
            RpcError::new(
                ErrorCode::SessionBusy,
                "the session is in a turn; ask again once it ends",
            )
        })?;
        // `None` once the daemon's stop or the idle unload took it.
        let runtime = runtime.as_ref().ok_or_else(not_loaded)?;
        let report = match params.report {
            SessionReport::Context => serde_json::to_value(runtime.context_report()),
            SessionReport::Hooks => {
                let report = runtime.reload_hooks().map_err(|error| {
                    RpcError::new(ErrorCode::InternalError, format!("{error:#}"))
                })?;
                serde_json::to_value(report)
            }
            _ => {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    "this daemon does not know the report",
                ))
            }
        };
        report
            .map(|report| SessionInspectResponse { report })
            .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))
    }
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
