//! Reads a report about a loaded session between its turns.

use rho_protocol::{
    ErrorCode, RpcError, SessionInspectParams, SessionInspectResponse, SessionReport,
    SessionReportData,
};

use super::Sessions;
use crate::app::{
    compaction_report::{session_compaction_report, CompactionReport},
    context_report::ContextReport,
    daemon_host::jobs::SessionJob,
};

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
        let runtime = session.runtime.lock().await;
        // `None` once the daemon's stop or the idle unload took it.
        let runtime = runtime.as_ref().ok_or_else(not_loaded)?;
        // NEXT_MAJOR(rho-protocol): replace the untyped SessionInspectResponse::report with the typed SessionReportData, render rho's TUI from it, and remove typed_report.
        // Until then each report goes out twice: as rho's frontends show it,
        // and typed.
        let (report, typed) = match params.report {
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
                let snapshot = runtime.reload_hooks().map_err(|error| {
                    RpcError::new(ErrorCode::InternalError, format!("{error:#}"))
                })?;
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
        report
            .map(|report| SessionInspectResponse {
                report,
                typed_report: Some(typed),
            })
            .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))
    }
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
