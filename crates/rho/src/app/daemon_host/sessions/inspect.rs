//! Reads a report about a loaded session between its turns, as rho's
//! frontends show it (`rho/session/inspect`) or typed (`rho/session/report`),
//! and reloads its hooks (`rho/session/hooks/reload`), the one change here.
//! A report changes nothing about the session it reads.

use std::sync::Arc;

use rho_protocol::{
    ErrorCode, RpcError, SessionHooksReloadParams, SessionHooksReloadResponse, SessionId,
    SessionInspectParams, SessionInspectResponse, SessionReport, SessionReportData,
    SessionReportParams, SessionReportResponse,
};

use super::{LoadedSession, Sessions};
use crate::app::{
    compaction_report::{session_compaction_report, CompactionReport},
    context_report::ContextReport,
    daemon_host::jobs::SessionJob,
};

/// One report both ways: the typed data and the presentation view derived
/// from it.
struct ReadReport {
    typed: SessionReportData,
    shown: serde_json::Value,
}

impl Sessions {
    pub(in crate::app::daemon_host) async fn inspect(
        &self,
        params: SessionInspectParams,
    ) -> Result<SessionInspectResponse, RpcError> {
        let read = self.read_report(&params.session, params.report).await?;
        Ok(SessionInspectResponse { report: read.shown })
    }

    pub(in crate::app::daemon_host) async fn report(
        &self,
        params: SessionReportParams,
    ) -> Result<SessionReportResponse, RpcError> {
        let read = self.read_report(&params.session, params.report).await?;
        Ok(SessionReportResponse::new(read.typed))
    }

    /// Reloads the session's hooks from their files. Files that do not load
    /// leave its hooks as they were, which the answer and its hooks report
    /// say, rather than failing the request.
    pub(in crate::app::daemon_host) async fn reload_hooks(
        &self,
        params: SessionHooksReloadParams,
    ) -> Result<SessionHooksReloadResponse, RpcError> {
        let session = self.between_turns(&params.session).await?;
        let runtime = session.runtime.lock().await;
        let runtime = runtime.as_ref().ok_or_else(not_loaded)?;
        let reloaded = match runtime.reload_hooks() {
            Ok(()) => true,
            Err(error) => {
                tracing::info!(
                    target: "rho::hooks",
                    session = %session.id,
                    "hooks were not reloaded: {error}"
                );
                false
            }
        };
        Ok(SessionHooksReloadResponse::new(reloaded))
    }

    // NEXT_MAJOR(rho-protocol): make rho/session/inspect answer the typed SessionReportData, render rho's TUI from it, and remove rho/session/report.
    // Until then both methods read the report here, and inspect answers
    // only the presentation view.
    async fn read_report(
        &self,
        session_id: &SessionId,
        report: SessionReport,
    ) -> Result<ReadReport, RpcError> {
        let session = self.between_turns(session_id).await?;
        let runtime = session.runtime.lock().await;
        // `None` once the daemon's stop or the idle unload took it.
        let runtime = runtime.as_ref().ok_or_else(not_loaded)?;
        let (shown, typed) = match report {
            SessionReport::Context => {
                let report = runtime.context_report();
                (
                    serde_json::to_value(ContextReport::from(&report)),
                    SessionReportData::Context(report),
                )
            }
            SessionReport::Compaction => {
                let report =
                    session_compaction_report(&runtime.compaction_diagnostics(&session.config()));
                (
                    serde_json::to_value(CompactionReport::from(&report)),
                    SessionReportData::Compaction(Box::new(report)),
                )
            }
            SessionReport::ProviderContext => {
                let auths = super::options::available_auths(runtime.credentials()).await;
                let report = runtime.provider_context_report(&session.config(), &auths);
                (
                    serde_json::to_value(&report),
                    SessionReportData::ProviderContext((&report).into()),
                )
            }
            SessionReport::Hooks => {
                let snapshot = runtime.hooks_report();
                (
                    serde_json::to_value(snapshot.report()),
                    SessionReportData::Hooks(snapshot.session_report()),
                )
            }
            _ => {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    "this daemon does not know the report",
                ))
            }
        };
        let shown =
            shown.map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))?;
        Ok(ReadReport { typed, shown })
    }

    /// The loaded session `session_id` names, unless a turn holds it. Its
    /// runtime is `None` once the daemon's stop or the idle unload took it.
    async fn between_turns(&self, session_id: &SessionId) -> Result<Arc<LoadedSession>, RpcError> {
        self.refuse_when_stopping()?;
        let session = self
            .loaded_session(session_id)
            .await?
            .ok_or_else(not_loaded)?;
        // A report reads what a turn is changing, and a client asked to
        // see it now: refused, not queued behind the turn. A change or an
        // options refresh holds the runtime only briefly, so those are
        // waited for.
        match self.jobs.session_job(&session.id) {
            SessionJob::Busy(job) if job == session.job => {
                return Err(RpcError::new(
                    ErrorCode::SessionBusy,
                    "the session is in a turn; ask again once it ends",
                ));
            }
            // Another job writes it, as a subagent run writes its own
            // session: not a turn of this one, but as busy.
            SessionJob::Busy(job) => {
                return Err(RpcError::new(
                    ErrorCode::SessionBusy,
                    format!(
                        "job {job} is writing session {}; ask again once it ends",
                        session.id
                    ),
                ));
            }
            SessionJob::New | SessionJob::Last(_) => {}
        }
        Ok(session)
    }
}

fn not_loaded() -> RpcError {
    RpcError::new(
        ErrorCode::SessionNotLoaded,
        "the daemon has not loaded the session; attach to it first",
    )
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
