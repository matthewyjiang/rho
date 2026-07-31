use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    event::{RunOutcome, ToolCompletion},
    hooks::{
        bounded_failure, error_label, AfterToolUsePayload, HookEventKind, HookPayload,
        HookStopReason, HookTool, HookToolStatus, HookWiring, RunCompletedPayload,
        RunFailedPayload,
    },
    Error, RunId, SessionId, ToolCallId,
};

/// Run-scoped hook identity and dispatch.
///
/// One value per run carries the session, run, and workspace identity every
/// envelope reports, so call sites deep in the tool batch do not have to rebuild
/// it or thread the whole runtime.
pub(super) struct RunHooks {
    hooks: HookWiring,
    session_id: SessionId,
    run_id: RunId,
    workspace_root: Option<PathBuf>,
}

impl RunHooks {
    pub(super) fn new(runtime: &crate::client::Rho, session_id: SessionId, run_id: RunId) -> Self {
        Self {
            hooks: runtime.hooks.clone(),
            session_id,
            run_id,
            workspace_root: runtime
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_path_buf()),
        }
    }

    pub(super) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Reports that one tool call resolved, successfully or not.
    ///
    /// Fired after the call's `ToolFinished` run event so hook order matches the
    /// order a host observes.
    pub(super) async fn after_tool_use(
        &self,
        tool_name: &str,
        call_id: &ToolCallId,
        completion: &ToolCompletion,
        duration: Option<Duration>,
    ) {
        let bounds = self.hooks.bounds();
        self.hooks
            .observe(
                HookEventKind::AfterToolUse,
                Some(&self.session_id),
                Some(&self.run_id),
                self.workspace_root(),
                |builder| {
                    let (status, failure) = match completion {
                        ToolCompletion::Success(_) => (HookToolStatus::Succeeded, None),
                        ToolCompletion::Failure(failure) => (
                            HookToolStatus::Failed,
                            Some(bounded_failure(
                                format!("{:?}", failure.kind()).to_lowercase(),
                                failure.message(),
                                bounds,
                                builder.truncation(),
                                "payload.failure.message",
                            )),
                        ),
                        ToolCompletion::Unavailable => (HookToolStatus::Unavailable, None),
                    };
                    HookPayload::AfterToolUse(AfterToolUsePayload {
                        tool: HookTool::new(tool_name, Some(call_id.as_str().to_owned())),
                        status,
                        failure,
                        duration_ms: duration.map(|elapsed| elapsed.as_millis() as u64),
                    })
                },
            )
            .await;
    }

    /// Reports the terminal result of the whole run exactly once.
    pub(super) async fn run_finished(&self, result: &Result<RunOutcome, Error>) {
        let bounds = self.hooks.bounds();
        match result {
            Ok(outcome) => {
                self.hooks
                    .observe(
                        HookEventKind::RunCompleted,
                        Some(&self.session_id),
                        Some(&self.run_id),
                        self.workspace_root(),
                        |_| {
                            HookPayload::RunCompleted(RunCompletedPayload {
                                stop_reason: HookStopReason::from(outcome.stop_reason()),
                                revision: outcome.revision().get(),
                            })
                        },
                    )
                    .await
            }
            Err(error) => {
                self.hooks
                    .observe(
                        HookEventKind::RunFailed,
                        Some(&self.session_id),
                        Some(&self.run_id),
                        self.workspace_root(),
                        |builder| {
                            HookPayload::RunFailed(RunFailedPayload {
                                failure: bounded_failure(
                                    error_label(error),
                                    &error.to_string(),
                                    bounds,
                                    builder.truncation(),
                                    "payload.failure.message",
                                ),
                            })
                        },
                    )
                    .await
            }
        }
    }
}
