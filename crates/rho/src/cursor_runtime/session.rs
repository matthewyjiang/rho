//! Execute a `runtime: cursor` delegated run via `cursor-agent -p`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::cli_runtime::{
    drain::DrainInput,
    session::{CliSessionOverrides, CliSessionPolicy, CliSessionRequest},
    status_sink::{RateLimitRecorder, RuntimeLabel, StatusSink},
    stream_effect::{StreamEffect, TerminalResult},
    terminal::{assess_terminal, TerminalOutcome},
    CliExecutable,
};
use crate::run_artifacts::AttachmentEvent;

use crate::{
    agent::{CursorTool, PromptPolicy},
    permission::PermissionMode,
    run_artifacts::RunArtifactIdentity,
    subagent::RunStatus,
};

use super::{
    auth::{self, CursorAuthError, CursorAuthStatus},
    executable,
    models::{CURSOR_LABEL, CURSOR_PROGRAM_LABEL},
    spawn::{self, CursorSpawnRequest},
    stream::CursorStreamMapper,
};

/// Inputs for one Cursor Agent subagent run, including bound runtime values.
///
/// `AgentExecutor` builds this directly after typed binding; tests build the
/// same shape and fill [`Self::overrides`].
pub(crate) struct CursorSessionRequest {
    /// Agent system prompt policy. The only definition field a spawn needs.
    pub(crate) system_prompt: PromptPolicy,
    /// Bound snapshot stamped onto `result.json`. Spawn reads model from here
    /// so it cannot drift from the Starting identity.
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) tools: Vec<CursorTool>,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// When set, the launcher already force-replaced `result.json` with this
    /// Starting status. The sink continues from it instead of rewriting.
    pub(crate) started_status: Option<RunStatus>,
    /// Auth preflight result. When set, production `auth::query` is not called.
    pub(crate) auth_status: Option<Result<CursorAuthStatus, CursorAuthError>>,
    pub(crate) overrides: CliSessionOverrides,
}

/// Run one Cursor Agent session to completion, writing the subagent contract.
pub(crate) async fn run_session(request: CursorSessionRequest) -> anyhow::Result<()> {
    let CursorSessionRequest {
        system_prompt,
        identity,
        tools,
        prompt,
        output_file,
        cwd,
        permission_mode,
        cancellation,
        status_tx,
        started_status,
        auth_status,
        overrides,
    } = request;
    let model = identity.model.clone();
    crate::cli_runtime::session::run_session(
        CliSessionRequest {
            identity,
            prompt: prompt.clone(),
            output_file,
            cancellation,
            status_tx,
            started_status,
            overrides,
        },
        CursorPolicy {
            system_prompt,
            model,
            tools,
            prompt,
            permission_mode,
            cwd,
            auth_status,
            drain_prompt: None,
        },
    )
    .await
}

struct CursorPolicy {
    system_prompt: PromptPolicy,
    model: Option<String>,
    tools: Vec<CursorTool>,
    prompt: String,
    permission_mode: PermissionMode,
    cwd: PathBuf,
    auth_status: Option<Result<CursorAuthStatus, CursorAuthError>>,
    drain_prompt: Option<String>,
}

impl CliSessionPolicy for CursorPolicy {
    type Mapper = CursorStreamMapper;

    fn label(&self) -> RuntimeLabel {
        CURSOR_LABEL
    }

    fn preflight(
        &mut self,
        sink: &mut StatusSink,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send {
        if let Some(warning) = unknown_cursor_model_warning(self.model.as_deref()) {
            tracing::warn!("{warning}");
            sink.apply_effect(StreamEffect::Attachment(AttachmentEvent::Notice(warning)));
        }
        let auth_status = self.auth_status.take();
        async move {
            // An unauthenticated cursor-agent may block rather than exit, so preflight first.
            let auth_result = match auth_status {
                Some(result) => result,
                None => auth::query().await,
            };
            match auth_result {
                Ok(status) if status.is_authenticated => Ok(()),
                Ok(status) => Err(status.auth_description()),
                Err(CursorAuthError::BinaryMissing) => {
                    Err(CursorAuthError::BinaryMissing.to_string())
                }
                Err(error) => Err(format!(
                    "{CURSOR_PROGRAM_LABEL}: auth preflight failed: {error}"
                )),
            }
        }
    }

    fn resolve_executable(&self) -> Result<CliExecutable, String> {
        executable::resolve().map_err(|error| error.to_string())
    }

    fn spawn_args(
        &mut self,
        _output_file: &Path,
        frozen: Option<Vec<String>>,
    ) -> Result<(Vec<OsString>, PathBuf), String> {
        let allowed = spawn::map_permission_mode(self.permission_mode, &self.tools)
            .map_err(|error| error.to_string())?;
        self.drain_prompt = Some(
            spawn::compose_prompt(&self.system_prompt, &self.prompt)
                .map_err(|error| error.to_string())?,
        );
        let mut plan = spawn::build_spawn_plan(&CursorSpawnRequest {
            model: self.model.clone(),
            allowed,
            cwd: self.cwd.clone(),
        });
        if let Some(arguments) = frozen {
            plan.args = spawn::apply_frozen_identity_args(plan.args, &arguments);
        }
        let session_id = uuid::Uuid::new_v4().to_string();
        let spawn_args = spawn::finalize_spawn_args(&plan, &session_id);
        Ok((spawn_args, plan.cwd))
    }

    fn log_path(&self, output_file: &Path) -> PathBuf {
        spawn::log_path(output_file)
    }

    fn drain_input(&mut self) -> DrainInput {
        DrainInput::Text {
            prompt: self
                .drain_prompt
                .take()
                .unwrap_or_else(|| self.prompt.clone()),
        }
    }

    fn mapper(&self) -> Self::Mapper {
        CursorStreamMapper::new()
    }

    fn assess_exit(
        &self,
        pending: Option<TerminalResult>,
        status: ExitStatus,
        log_tail: &str,
    ) -> TerminalOutcome {
        assess_terminal(pending, status, log_tail, CURSOR_PROGRAM_LABEL)
    }

    fn rate_limit_recorder(&self) -> Option<Box<dyn RateLimitRecorder>> {
        None
    }
}

/// Warn when a pinned Cursor model is missing from a non-empty cache.
fn unknown_cursor_model_warning(model: Option<&str>) -> Option<String> {
    let model = model.filter(|value| !value.is_empty())?;
    let cached = super::models::cached();
    if cached.is_empty() {
        return None;
    }
    let lookup = model.split_once('[').map(|(id, _)| id).unwrap_or(model);
    if cached.iter().any(|row| row.id == lookup) {
        return None;
    }
    Some(format!("cursor model '{model}' is not in the cached list"))
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
