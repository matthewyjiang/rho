//! Execute a `runtime: claude-cli` delegated run via `claude -p`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::cli_runtime::{
    session::{CliSessionOverrides, CliSessionPolicy, CliSessionRequest},
    CliExecutable,
};
#[cfg(test)]
use crate::subagent;

use crate::{
    agent::PromptPolicy, permission::PermissionMode, run_artifacts::RunArtifactIdentity,
    subagent::RunStatus,
};

use super::{
    auth::{self, ClaudeAuthError, ClaudeAuthStatus},
    drain::DrainInput,
    executable,
    persist::{RuntimeLabel, CLAUDE_LABEL},
    spawn::{self, ClaudeSpawnRequest},
    stream::{StreamMapper, TerminalResult},
    terminal::{assess_terminal, TerminalOutcome},
};

/// Inputs for one Claude CLI subagent run, including bound runtime values.
///
/// `AgentExecutor` builds this directly after typed binding; tests build the
/// same shape and fill [`Self::overrides`].
pub(crate) struct ClaudeSessionRequest {
    /// Agent system prompt policy. The only definition field a spawn needs.
    pub(crate) system_prompt: PromptPolicy,
    /// Bound snapshot stamped onto `result.json`. Spawn reads model and
    /// reasoning from here so they cannot drift from the Starting identity.
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    /// Exact `--max-turns` value. Always set from the configured/definition step
    /// cap at bind/launch time; never recomputed inside the session adapter.
    pub(crate) max_turns: u64,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// When set, the launcher already force-replaced `result.json` with this
    /// Starting status. The sink continues from it instead of rewriting.
    pub(crate) started_status: Option<RunStatus>,
    /// Parent→child plain-text messages. Present for interactive parent sessions.
    pub(crate) parent_messages: Option<super::messaging::ClaudeMessageInbox>,
    /// Auth preflight result. When set, production `auth::query` is not called.
    pub(crate) auth_status: Option<Result<ClaudeAuthStatus, ClaudeAuthError>>,
    /// Rate-limit cache path. Tests inject a temp path so settle never touches
    /// the host default cache.
    pub(crate) rate_limit_state_path: Option<PathBuf>,
    pub(crate) overrides: CliSessionOverrides,
}

/// Run one Claude CLI session to completion, writing the subagent contract.
pub(crate) async fn run_session(request: ClaudeSessionRequest) -> anyhow::Result<()> {
    let ClaudeSessionRequest {
        system_prompt,
        identity,
        tools,
        inherit_claude_config,
        max_turns,
        prompt,
        output_file,
        cwd,
        permission_mode,
        cancellation,
        status_tx,
        started_status,
        parent_messages,
        auth_status,
        rate_limit_state_path,
        overrides,
    } = request;
    let model = identity.model.clone();
    let reasoning = identity.reasoning;
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
        ClaudePolicy {
            system_prompt,
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            cwd,
            max_turns,
            reasoning,
            parent_messages,
            prompt,
            auth_status,
            rate_limit_state_path,
        },
    )
    .await
}

struct ClaudePolicy {
    system_prompt: PromptPolicy,
    model: Option<String>,
    tools: Vec<String>,
    inherit_claude_config: bool,
    permission_mode: PermissionMode,
    cwd: PathBuf,
    max_turns: u64,
    reasoning: Option<crate::agent::ReasoningLevel>,
    parent_messages: Option<super::messaging::ClaudeMessageInbox>,
    prompt: String,
    auth_status: Option<Result<ClaudeAuthStatus, ClaudeAuthError>>,
    rate_limit_state_path: Option<PathBuf>,
}

impl CliSessionPolicy for ClaudePolicy {
    type Mapper = StreamMapper;

    fn label(&self) -> RuntimeLabel {
        CLAUDE_LABEL
    }

    fn preflight(
        &mut self,
        _sink: &mut super::persist::StatusSink,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send {
        let auth_status = self.auth_status.take();
        async move {
            // An unauthenticated claude may block rather than exit, so preflight first.
            let auth_result = match auth_status {
                Some(result) => result,
                None => auth::query().await,
            };
            match auth_result {
                Ok(status) if status.logged_in => Ok(()),
                Ok(_) => Err("claude code: not signed in - run /login claude-code".into()),
                Err(ClaudeAuthError::BinaryMissing) => {
                    Err(ClaudeAuthError::BinaryMissing.to_string())
                }
                Err(error) => Err(format!("claude code: auth preflight failed: {error}")),
            }
        }
    }

    fn resolve_executable(&self) -> Result<CliExecutable, String> {
        executable::resolve().map_err(|error| error.to_string())
    }

    fn spawn_args(
        &mut self,
        output_file: &Path,
        frozen: Option<Vec<String>>,
    ) -> Result<(Vec<OsString>, PathBuf), String> {
        // Always map and generate permission argv from the effective bound mode
        // and bound tools. Frozen argv must not skip that gate or validate an
        // empty tool list / stale Bypass mode.
        let permission_mode = spawn::map_permission_mode(
            self.permission_mode,
            &self.tools,
            self.inherit_claude_config,
        )
        .map_err(|error| error.to_string())?;
        let mut plan = spawn::build_spawn_plan(&ClaudeSpawnRequest {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            tools: self.tools.clone(),
            inherit_claude_config: self.inherit_claude_config,
            permission_mode,
            cwd: self.cwd.clone(),
            max_turns: self.max_turns,
            reasoning: self.reasoning,
            // Delegated runs publish a resumable Claude session id.
            session_persistence: spawn::SessionPersistence::Keep,
            input_format: spawn::ClaudeInputFormat::StreamJson,
        });
        if let Some(arguments) = frozen {
            plan.args =
                spawn::apply_frozen_identity_args(plan.args, &ensure_stream_json_input(arguments));
        }

        // Materialize the system prompt next to the status file (kept as a run
        // artifact). File flags keep multiline prompt bytes out of shell/cmd argv.
        let spawn_args =
            spawn::finalize_spawn_args(&plan, output_file).map_err(|error| error.to_string())?;
        Ok((spawn_args, plan.cwd))
    }

    fn log_path(&self, output_file: &Path) -> PathBuf {
        spawn::log_path(output_file)
    }

    fn drain_input(&mut self) -> DrainInput {
        DrainInput::StreamJson {
            initial_prompt: self.prompt.clone(),
            parent_messages: self.parent_messages.take(),
        }
    }

    fn mapper(&self) -> Self::Mapper {
        StreamMapper::new()
    }

    fn assess_exit(
        &self,
        pending: Option<TerminalResult>,
        status: ExitStatus,
        log_tail: &str,
    ) -> TerminalOutcome {
        if !status.success() && spawn::looks_like_max_turns_unsupported(log_tail) {
            return TerminalOutcome::Failure {
                terminal: pending,
                detail: format!(
                    "{}: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap",
                    CLAUDE_LABEL.program
                ),
                prefer_detail: true,
            };
        }
        assess_terminal(pending, status, log_tail, CLAUDE_LABEL.program)
    }

    fn rate_limit_state_path(&self) -> Option<PathBuf> {
        self.rate_limit_state_path.clone()
    }
}

/// Ensures frozen Claude argv can accept parent messages over stream-json stdin.
fn ensure_stream_json_input(mut arguments: Vec<String>) -> Vec<String> {
    let has_stream_json = arguments
        .windows(2)
        .any(|window| window[0] == "--input-format" && window[1] == "stream-json");
    if !has_stream_json {
        arguments.push("--input-format".into());
        arguments.push("stream-json".into());
    }
    arguments
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
