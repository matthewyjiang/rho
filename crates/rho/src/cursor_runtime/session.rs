//! Execute a `runtime: cursor` delegated run via `cursor-agent -p`.

use std::process::Stdio;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::claude_runtime::{
    drain::{self, DrainEnd},
    line_decoder::MAX_NDJSON_LINE_BYTES,
    persist::StatusSink,
    stream::TerminalResult,
    terminal::{assess_terminal, TerminalOutcome},
};
use crate::cli_runtime::{read_log_tail, CliExecutable, OwnedChild};

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
    spawn::{self, CursorSpawnPlan, CursorSpawnRequest},
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
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// When set, the launcher already force-replaced `result.json` with this
    /// Starting status. The sink continues from it instead of rewriting.
    pub(crate) started_status: Option<RunStatus>,
    pub(crate) overrides: CursorSessionOverrides,
}

/// Seams a session may replace. Production leaves every field unset.
#[derive(Default)]
pub(crate) struct CursorSessionOverrides {
    /// Cursor binary to run instead of the resolved one.
    pub(crate) executable: Option<CliExecutable>,
    /// Frozen workflow identity arguments. Permission-sensitive flags are not
    /// reused as-is; launch regenerates those from the effective bound mode.
    pub(crate) frozen_argv: Option<Vec<String>>,
    /// Auth preflight result. When set, production `auth::query` is not called.
    pub(crate) auth_status: Option<Result<CursorAuthStatus, CursorAuthError>>,
    /// Title generated for this run. The artifact sink reads it; the watch
    /// channel stays one-way.
    pub(crate) live_title: Option<crate::run_artifacts::LiveRunTitle>,
    /// A frozen caller can verify process facts and configure the child at the spawn boundary.
    pub(crate) before_spawn: Option<BeforeSpawn>,
}

pub(crate) type BeforeSpawn =
    Box<dyn Fn(&mut tokio::process::Command) -> std::io::Result<()> + Send + Sync>;

/// Run one Cursor Agent session to completion, writing the subagent contract.
pub(crate) async fn run_session(mut request: CursorSessionRequest) -> anyhow::Result<()> {
    let mut sink = match request.started_status.take() {
        Some(status) => StatusSink::continue_from(
            request.output_file.clone(),
            status,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.live_title.clone(),
            /*rate_limit_state_path*/ None,
            CURSOR_LABEL,
        )?,
        None => StatusSink::new(
            request.output_file.clone(),
            &request.identity,
            &request.prompt,
            request.status_tx.take(),
            /*rate_limit_state_path*/ None,
            CURSOR_LABEL,
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
    /// apply usage while reporting cancelled state.
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
            if !sink.status().state.is_terminal() {
                let pending = pending.map(|terminal| *terminal);
                match assess_terminal(pending, status, &log_tail, CURSOR_PROGRAM_LABEL) {
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

/// Preflight, spawn, and drain one Cursor run without writing terminal state.
async fn drive_session(
    request: &mut CursorSessionRequest,
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
    executable: CliExecutable,
    plan: CursorSpawnPlan,
    spawn_args: Vec<std::ffi::OsString>,
    log_path: std::path::PathBuf,
    log_file: std::fs::File,
    prompt: String,
}

/// Check auth, resolve the binary, and materialize argv plus the log file.
///
/// Every failure is already user-facing text; the caller turns it into
/// [`crate::subagent::RunState::Error`].
async fn prepare_launch(request: &mut CursorSessionRequest) -> Result<Launch, String> {
    // An unauthenticated cursor-agent may block rather than exit, so preflight first.
    let auth_result = match request.overrides.auth_status.take() {
        Some(result) => result,
        None => auth::query().await,
    };
    match auth_result {
        Ok(status) if status.is_authenticated => {}
        Ok(status) => return Err(status.auth_description()),
        Err(CursorAuthError::BinaryMissing) => {
            return Err(CursorAuthError::BinaryMissing.to_string())
        }
        Err(error) => {
            return Err(format!(
                "{CURSOR_PROGRAM_LABEL}: auth preflight failed: {error}"
            ))
        }
    }

    let executable = match request.overrides.executable.take() {
        Some(executable) => executable,
        None => executable::resolve().map_err(|error| error.to_string())?,
    };

    let frozen_arguments = request.overrides.frozen_argv.take();
    let allowed = spawn::map_permission_mode(request.permission_mode, &request.tools)
        .map_err(|error| error.to_string())?;
    let prompt = spawn::compose_prompt(&request.system_prompt, &request.prompt)
        .map_err(|error| error.to_string())?;
    let mut plan = spawn::build_spawn_plan(&CursorSpawnRequest {
        model: request.identity.model.clone(),
        allowed,
        cwd: request.cwd.clone(),
    });
    if let Some(arguments) = frozen_arguments {
        plan.args = spawn::apply_frozen_identity_args(plan.args, &arguments);
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let spawn_args = spawn::finalize_spawn_args(&plan, &session_id);

    let log_path = spawn::log_path(&request.output_file);
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .map_err(|error| format!("{CURSOR_PROGRAM_LABEL}: could not open log file: {error}"))?
        .into_std()
        .await;

    Ok(Launch {
        executable,
        plan,
        spawn_args,
        log_path,
        log_file,
        prompt,
    })
}

/// Spawn the child and drain it, leaving no live process tree behind.
async fn run_child(
    request: &mut CursorSessionRequest,
    sink: &mut StatusSink,
    launch: Launch,
) -> SessionOutcome {
    let Launch {
        executable,
        plan,
        spawn_args,
        log_path,
        log_file,
        prompt,
    } = launch;

    let mut command = match executable.try_command(&spawn_args) {
        Ok(command) => command,
        Err(error) => return SessionOutcome::Failed(format!("{CURSOR_PROGRAM_LABEL}: {error}")),
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
                "{CURSOR_PROGRAM_LABEL}: frozen executable changed before spawn: {error}"
            ));
        }
    }

    let mut child = match OwnedChild::spawn(command) {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SessionOutcome::Failed(CursorAuthError::BinaryMissing.to_string());
        }
        Err(error) => {
            return SessionOutcome::Failed(format!(
                "{CURSOR_PROGRAM_LABEL}: failed to spawn `{}`: {error}",
                executable.display()
            ));
        }
    };

    let outcome = drain_child(request, sink, &mut child, &log_path, prompt).await;
    if !matches!(outcome, SessionOutcome::Exited { .. }) {
        child.terminate().await;
    }
    outcome
}

/// Write the prompt, map stdout, and wait for exit.
async fn drain_child(
    request: &mut CursorSessionRequest,
    sink: &mut StatusSink,
    child: &mut OwnedChild,
    log_path: &std::path::Path,
    prompt: String,
) -> SessionOutcome {
    sink.mark_running();

    let drained = {
        let mut mapper = CursorStreamMapper::new();
        let mut on_effect = |effect| sink.apply_effect(effect);
        drain::drain_child(
            child,
            drain::DrainConfig {
                program_label: CURSOR_PROGRAM_LABEL,
                // Largest observed Cursor NDJSON line was 100,037 B; 4 MiB is
                // ~40× headroom with the same runaway-guard role as Claude.
                max_line_bytes: MAX_NDJSON_LINE_BYTES,
            },
            &mut mapper,
            drain::DrainInput::Text { prompt },
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
        DrainEnd::Exited(Err(error)) => SessionOutcome::Failed(format!(
            "{CURSOR_PROGRAM_LABEL}: failed waiting for child: {error}"
        )),
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
