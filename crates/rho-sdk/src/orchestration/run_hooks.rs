use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    event::{RunOutcome, ToolCompletion},
    hooks::{
        bounded_failure, error_label, BoundedFailure, HookPayload, HookStopReason,
        HookToolIdentity, HookToolStatus, HookWiring, RunCompletedPayload, RunFailedPayload,
    },
    tool::ToolErrorKind,
    Error, RunId, SessionId, ToolCallId,
};

const fn tool_error_label(kind: ToolErrorKind) -> &'static str {
    match kind {
        ToolErrorKind::InvalidArguments => "invalid_arguments",
        ToolErrorKind::Execution => "execution",
        ToolErrorKind::PolicyDenied => "policy_denied",
        ToolErrorKind::Cancelled => "cancelled",
    }
}

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
    pub(super) fn after_tool_use(
        &self,
        tool_name: &str,
        call_id: &ToolCallId,
        completion: &ToolCompletion,
        duration: Option<Duration>,
    ) {
        let (status, failure) = match completion {
            ToolCompletion::Success(_) => (HookToolStatus::Succeeded, None),
            ToolCompletion::Failure(failure) => (
                HookToolStatus::Failed,
                Some(BoundedFailure {
                    kind: tool_error_label(failure.kind()),
                    message: failure.message(),
                    field: "payload.failure",
                }),
            ),
            ToolCompletion::Unavailable => (HookToolStatus::Unavailable, None),
        };
        self.hooks.observe_after_tool_use(
            HookToolIdentity {
                session_id: Some(&self.session_id),
                run_id: Some(&self.run_id),
                workspace_root: self.workspace_root(),
                tool_name,
                call_id,
            },
            status,
            failure,
            duration.map(|elapsed| elapsed.as_millis() as u64),
        );
    }

    /// Reports the terminal result of the whole run exactly once.
    pub(super) fn run_finished(&self, result: &Result<RunOutcome, Error>) {
        let bounds = self.hooks.bounds();
        match result {
            Ok(outcome) => self.hooks.observe(
                Some(&self.session_id),
                Some(&self.run_id),
                self.workspace_root(),
                |_| {
                    HookPayload::RunCompleted(RunCompletedPayload {
                        stop_reason: HookStopReason::from(outcome.stop_reason()),
                        revision: outcome.revision().get(),
                    })
                },
            ),
            Err(error) => self.hooks.observe(
                Some(&self.session_id),
                Some(&self.run_id),
                self.workspace_root(),
                |builder| {
                    HookPayload::RunFailed(RunFailedPayload {
                        failure: bounded_failure(
                            BoundedFailure {
                                kind: error_label(error),
                                message: &error.to_string(),
                                field: "payload.failure",
                            },
                            bounds,
                            builder.truncation(),
                        ),
                    })
                },
            ),
        }
    }
}
