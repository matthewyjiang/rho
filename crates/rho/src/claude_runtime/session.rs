//! Execute a `runtime: claude-cli` delegated run via `claude -p`.

use std::process::Stdio;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

#[cfg(test)]
use crate::subagent;

use crate::{agent::PromptPolicy, permission::PermissionMode, subagent::RunStatus};

use super::{
    auth::{self, ClaudeAuthError, ClaudeAuthStatus},
    child::OwnedChild,
    drain::{self, DrainEnd},
    executable::{self, ClaudeExecutable},
    persist::StatusSink,
    spawn::{self, ClaudeSpawnPlan, ClaudeSpawnRequest},
    stream::TerminalResult,
    terminal::{assess_terminal, TerminalOutcome},
};

pub(crate) use super::persist::ClaudeRunIdentity;

/// Inputs for one Claude CLI subagent run, including bound runtime values.
///
/// `AgentExecutor` builds this directly after typed binding; tests build the
/// same shape and fill [`Self::overrides`].
pub(crate) struct ClaudeSessionRequest {
    /// Agent system prompt policy. The only definition field a spawn needs.
    pub(crate) system_prompt: PromptPolicy,
    pub(crate) identity: ClaudeRunIdentity,
    /// Bound Claude `--model`. `None` means omit the flag.
    pub(crate) model: Option<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    /// Exact `--max-turns` value. Always set from the configured/definition step
    /// cap at bind/launch time; never recomputed inside the session adapter.
    pub(crate) max_turns: u64,
    /// Claude `--effort` from definition `reasoning:`. `None` omits the flag.
    pub(crate) effort: Option<&'static str>,
    pub(crate) prompt: String,
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// When set, the launcher already force-replaced `result.json` with this
    /// Starting status. The sink continues from it instead of rewriting.
    pub(crate) started_status: Option<RunStatus>,
    /// Parent→child plain-text messages. Present for interactive parent sessions.
    pub(crate) parent_messages: Option<super::messaging::ClaudeMessageInbox>,
    pub(crate) overrides: ClaudeSessionOverrides,
}

impl ClaudeSessionRequest {
    /// Replace generated Claude arguments with the exact frozen workflow argv.
    pub(crate) fn set_frozen_argv(&mut self, arguments: Vec<String>) {
        self.overrides.frozen_argv = Some(arguments);
    }
}

/// Seams a session may replace. Production leaves every field unset.
#[derive(Default)]
pub(crate) struct ClaudeSessionOverrides {
    /// Claude binary to run instead of the resolved one.
    pub(crate) executable: Option<ClaudeExecutable>,
    /// Exact workflow arguments to use instead of generated Claude arguments.
    pub(crate) frozen_argv: Option<Vec<String>>,
    /// Auth preflight result. When set, production `auth::query` is not called.
    pub(crate) auth_status: Option<Result<ClaudeAuthStatus, ClaudeAuthError>>,
    /// Rate-limit cache path. Tests inject a temp path so settle never touches
    /// the host default cache.
    pub(crate) rate_limit_state_path: Option<std::path::PathBuf>,
    /// A frozen caller can verify process facts and configure the child at the spawn boundary.
    pub(crate) before_spawn: Option<BeforeSpawn>,
}

pub(crate) type BeforeSpawn =
    Box<dyn Fn(&mut tokio::process::Command) -> std::io::Result<()> + Send + Sync>;

/// Run one Claude CLI session to completion, writing the subagent contract.
pub(crate) async fn run_session(mut request: ClaudeSessionRequest) -> anyhow::Result<()> {
    if request.identity.model.is_none() {
        request.identity.model = request.model.clone();
    }
    let mut sink = match request.started_status.take() {
        Some(status) => StatusSink::continue_from(
            request.output_file.clone(),
            status,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.rate_limit_state_path.clone(),
        )?,
        None => StatusSink::new(
            request.output_file.clone(),
            &request.identity,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.rate_limit_state_path.clone(),
        )?,
    };
    let outcome = drive_session(&mut request, &mut sink).await;
    settle(sink, outcome).await;
    Ok(())
}

/// What one session decided, before any terminal artifact was written.
enum SessionOutcome {
    /// Cancellation observed. The reason becomes the stop activity.
    ///
    /// When a terminal `result` already arrived, keep it so settle can still
    /// apply turns/usage/cost while reporting cancelled state.
    Cancelled {
        reason: &'static str,
        pending: Option<Box<TerminalResult>>,
    },
    /// Setup, stdin, or stream failure. Ignored when the stream already
    /// published a terminal state.
    Failed(String),
    /// The child ran and was reaped. Final Ok/Error combines pending terminal
    /// metadata with the exit status.
    Exited {
        pending: Option<Box<TerminalResult>>,
        status: std::process::ExitStatus,
        log_tail: String,
    },
}

/// Write exactly one terminal artifact for `outcome`.
///
/// Every exit path in [`drive_session`] funnels through here, so "one terminal
/// write" is structural instead of repeated per branch.
async fn settle(mut sink: StatusSink, outcome: SessionOutcome) {
    match outcome {
        SessionOutcome::Cancelled { reason, pending } => {
            sink.stop(reason, pending.as_deref()).await
        }
        SessionOutcome::Failed(error) => sink.fail(error).await,
        SessionOutcome::Exited {
            pending,
            status,
            log_tail,
        } => {
            // Protocol type:error is pending metadata only; final Failed/Completed
            // is chosen here after exit. Leave already-terminal state alone.
            if !sink.status().state.is_terminal() {
                match assess_terminal(pending.map(|terminal| *terminal), status, &log_tail) {
                    TerminalOutcome::Success(terminal) => {
                        sink.finalize_success_from_stream(&terminal).await;
                    }
                    TerminalOutcome::Failure {
                        terminal,
                        detail,
                        prefer_detail,
                    } => {
                        sink.finalize_failure_from_stream(terminal.as_ref(), detail, prefer_detail)
                            .await;
                    }
                }
            }
        }
    }
}

/// Preflight, spawn, and drain one Claude run without writing terminal state.
async fn drive_session(
    request: &mut ClaudeSessionRequest,
    sink: &mut StatusSink,
) -> SessionOutcome {
    if request.cancellation.is_cancelled() {
        return SessionOutcome::Cancelled {
            reason: "cancelled before execution",
            pending: None,
        };
    }
    match prepare_launch(request).await {
        Ok(launch) => run_child(request, sink, launch).await,
        Err(error) => SessionOutcome::Failed(error),
    }
}

/// Everything needed to spawn, resolved before the child exists.
struct Launch {
    executable: ClaudeExecutable,
    plan: ClaudeSpawnPlan,
    spawn_args: Vec<std::ffi::OsString>,
    log_path: std::path::PathBuf,
    log_file: std::fs::File,
}

/// Check auth, resolve the binary, and materialize argv plus the log file.
///
/// Every failure is already user-facing text; the caller turns it into
/// [`RunState::Error`](crate::subagent::RunState::Error).
async fn prepare_launch(request: &mut ClaudeSessionRequest) -> Result<Launch, String> {
    // An unauthenticated claude may block rather than exit, so preflight first.
    let auth_result = match request.overrides.auth_status.take() {
        Some(result) => result,
        None => auth::query().await,
    };
    match auth_result {
        Ok(status) if status.logged_in => {}
        Ok(_) => return Err("claude code: not signed in - run /login claude-code".into()),
        Err(ClaudeAuthError::BinaryMissing) => {
            return Err(ClaudeAuthError::BinaryMissing.to_string())
        }
        Err(error) => return Err(format!("claude code: auth preflight failed: {error}")),
    }

    let executable = match request.overrides.executable.take() {
        Some(executable) => executable,
        None => executable::resolve().map_err(|error| error.to_string())?,
    };

    let frozen_arguments = request.overrides.frozen_argv.take();
    let permission_mode = spawn::map_permission_mode(
        request.permission_mode,
        &request.tools,
        request.inherit_claude_config,
    )
    .map_err(|error| error.to_string())?;
    let mut plan = spawn::build_spawn_plan(&ClaudeSpawnRequest {
        system_prompt: request.system_prompt.clone(),
        model: request.model.clone(),
        tools: if frozen_arguments.is_some() {
            Vec::new()
        } else {
            request.tools.clone()
        },
        inherit_claude_config: request.inherit_claude_config,
        permission_mode,
        cwd: request.cwd.clone(),
        max_turns: request.max_turns,
        effort: request.effort,
        // Delegated runs publish a resumable Claude session id.
        session_persistence: spawn::SessionPersistence::Keep,
        input_format: spawn::ClaudeInputFormat::StreamJson,
    });
    if let Some(arguments) = frozen_arguments {
        plan.args = arguments;
    }

    // Materialize the system prompt next to the status file (kept as a run
    // artifact). File flags keep multiline prompt bytes out of shell/cmd argv.
    let spawn_args = spawn::finalize_spawn_args(&plan, &request.output_file)
        .map_err(|error| error.to_string())?;

    let log_path = spawn::log_path(&request.output_file);
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .map_err(|error| format!("could not open claude log file: {error}"))?
        .into_std()
        .await;

    Ok(Launch {
        executable,
        plan,
        spawn_args,
        log_path,
        log_file,
    })
}

/// Spawn the child and drain it, leaving no live process tree behind.
async fn run_child(
    request: &mut ClaudeSessionRequest,
    sink: &mut StatusSink,
    launch: Launch,
) -> SessionOutcome {
    let Launch {
        executable,
        plan,
        spawn_args,
        log_path,
        log_file,
    } = launch;

    // Typed fallible builder: Windows shim validation becomes RunState::Error
    // before spawn instead of a generic I/O failure at CreateProcess.
    let mut command = match executable.try_command(&spawn_args) {
        Ok(command) => command,
        Err(error) => return SessionOutcome::Failed(error.to_string()),
    };
    command
        .current_dir(&plan.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log_file)
        .kill_on_drop(true);

    if let Some(before_spawn) = request.overrides.before_spawn.as_ref() {
        if let Err(error) = before_spawn(&mut command) {
            return SessionOutcome::Failed(format!(
                "claude code: frozen executable changed before spawn: {error}"
            ));
        }
    }

    let mut child = match OwnedChild::spawn(command) {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SessionOutcome::Failed(ClaudeAuthError::BinaryMissing.to_string());
        }
        Err(error) => {
            return SessionOutcome::Failed(format!(
                "claude code: failed to spawn `{}`: {error}",
                executable.display()
            ));
        }
    };

    let outcome = drain_child(request, sink, &mut child, &log_path).await;
    // Only a reaped exit guarantees the tree is gone; every other outcome
    // leaves the child mid-protocol.
    if !matches!(outcome, SessionOutcome::Exited { .. }) {
        child.terminate().await;
    }
    outcome
}

/// Write the prompt, map stdout, and wait for exit.
///
/// The drain itself is shared with the one-shot path; session only decides what
/// each end means for the run's status file.
async fn drain_child(
    request: &mut ClaudeSessionRequest,
    sink: &mut StatusSink,
    child: &mut OwnedChild,
    log_path: &std::path::Path,
) -> SessionOutcome {
    sink.mark_running();

    let parent_messages = request.parent_messages.take();

    // Stderr is a log file here, so the drain captures none of it.
    let drained = {
        let mut on_effect = |effect| sink.apply_effect(effect);
        drain::drain_child(
            child,
            drain::DrainInput::StreamJson {
                initial_prompt: request.prompt.clone(),
                parent_messages,
            },
            &request.cancellation,
            &mut on_effect,
        )
        .await
    };

    let pending = drained.terminal.map(Box::new);
    match drained.end {
        DrainEnd::Cancelled => SessionOutcome::Cancelled {
            reason: "cancelled",
            pending,
        },
        DrainEnd::StdinFailed(error) | DrainEnd::StreamFailed(error) => {
            SessionOutcome::Failed(error)
        }
        DrainEnd::Exited(Ok(status)) => SessionOutcome::Exited {
            pending,
            status,
            log_tail: read_log_tail(log_path).await,
        },
        DrainEnd::Exited(Err(error)) => {
            SessionOutcome::Failed(format!("claude code: failed waiting for child: {error}"))
        }
    }
}

async fn read_log_tail(path: &std::path::Path) -> String {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return String::new();
    };
    let trimmed = contents.trim();
    if trimmed.len() <= 400 {
        return trimmed.to_string();
    }
    let cut = trimmed.len() - 400;
    let boundary = rho_sdk::ceil_char_boundary(trimmed, cut);
    format!("{}{}", rho_sdk::ELLIPSIS, &trimmed[boundary..])
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
