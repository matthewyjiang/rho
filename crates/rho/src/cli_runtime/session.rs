//! Shared driver for one external CLI subagent session.
//!
//! Owns preflight ordering, executable resolution, spawn, drain,
//! terminate-on-non-exit, and the single terminal artifact write. Each CLI
//! supplies a [`CliSessionPolicy`] for the pieces that actually differ.

use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::claude_runtime::{
    drain::{self, DrainEnd, DrainInput, StreamLineMapper},
    line_decoder::MAX_NDJSON_LINE_BYTES,
    persist::{RuntimeLabel, StatusSink},
    stream::TerminalResult,
    terminal::TerminalOutcome,
};
use crate::run_artifacts::RunArtifactIdentity;
use crate::subagent::RunStatus;

use super::{read_log_tail, CliExecutable, OwnedChild};

/// Seams a session may replace. Production leaves every field unset.
#[derive(Default)]
pub(crate) struct CliSessionOverrides {
    /// Binary to run instead of the resolved one.
    pub(crate) executable: Option<CliExecutable>,
    /// Frozen workflow identity arguments. Permission-sensitive flags are not
    /// reused as-is; launch regenerates those from the effective bound mode.
    pub(crate) frozen_argv: Option<Vec<String>>,
    /// Title generated for this run. The artifact sink reads it; the watch
    /// channel stays one-way.
    pub(crate) live_title: Option<crate::run_artifacts::LiveRunTitle>,
    /// A frozen caller can verify process facts and configure the child at the spawn boundary.
    pub(crate) before_spawn: Option<BeforeSpawn>,
}

pub(crate) type BeforeSpawn =
    Box<dyn Fn(&mut tokio::process::Command) -> std::io::Result<()> + Send + Sync>;

/// Shared inputs every CLI session needs to open a sink and drive a child.
pub(crate) struct CliSessionRequest {
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    pub(crate) started_status: Option<RunStatus>,
    pub(crate) overrides: CliSessionOverrides,
}

/// One external CLI's session policy. The shared driver owns preflight ordering,
/// executable resolution, spawn, drain, terminate-on-non-exit, and the single
/// terminal artifact write. Implementors supply only what differs per CLI.
pub(crate) trait CliSessionPolicy: Send {
    type Mapper: StreamLineMapper;

    /// Sink label and `program` prefix for spawn/drain errors.
    fn label(&self) -> RuntimeLabel;

    /// Auth (and any other) checks that must finish before spawn.
    fn preflight(
        &mut self,
        sink: &mut StatusSink,
    ) -> impl Future<Output = Result<(), String>> + Send;

    fn resolve_executable(&self) -> Result<CliExecutable, String>;

    fn spawn_args(
        &mut self,
        output_file: &Path,
        frozen: Option<Vec<String>>,
    ) -> Result<(Vec<OsString>, PathBuf), String>;

    fn log_path(&self, output_file: &Path) -> PathBuf;

    fn drain_input(&mut self) -> DrainInput;

    fn mapper(&self) -> Self::Mapper;

    fn assess_exit(
        &self,
        pending: Option<TerminalResult>,
        status: ExitStatus,
        log_tail: &str,
    ) -> TerminalOutcome;

    fn rate_limit_state_path(&self) -> Option<PathBuf>;
}

/// Run one CLI session to completion, writing the subagent contract.
pub(crate) async fn run_session<P: CliSessionPolicy>(
    mut request: CliSessionRequest,
    mut policy: P,
) -> anyhow::Result<()> {
    let mut sink = match request.started_status.take() {
        Some(status) => StatusSink::continue_from(
            request.output_file.clone(),
            status,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.live_title.clone(),
            policy.rate_limit_state_path(),
            policy.label(),
        )?,
        None => StatusSink::new(
            request.output_file.clone(),
            &request.identity,
            &request.prompt,
            request.status_tx.take(),
            policy.rate_limit_state_path(),
            policy.label(),
        )?,
    };
    let outcome = drive_session(&mut request, &mut policy, &mut sink).await;
    settle(&policy, sink, outcome).await;
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
        status: ExitStatus,
        log_tail: String,
    },
}

/// Write exactly one terminal artifact for `outcome`.
///
/// Every exit path in [`drive_session`] funnels through here, so "one terminal
/// write" is structural instead of repeated per branch.
async fn settle<P: CliSessionPolicy>(policy: &P, mut sink: StatusSink, outcome: SessionOutcome) {
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
                let pending = pending.map(|terminal| *terminal);
                match policy.assess_exit(pending, status, &log_tail) {
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

/// Preflight, spawn, and drain one run without writing terminal state.
async fn drive_session<P: CliSessionPolicy>(
    request: &mut CliSessionRequest,
    policy: &mut P,
    sink: &mut StatusSink,
) -> SessionOutcome {
    if request.cancellation.is_cancelled() {
        return SessionOutcome::Cancelled {
            reason: "cancelled before execution",
            pending: None,
        };
    }
    match prepare_launch(request, policy, sink).await {
        Ok(launch) => run_child(request, policy, sink, launch).await,
        Err(error) => SessionOutcome::Failed(error),
    }
}

/// Everything needed to spawn, resolved before the child exists.
struct Launch {
    executable: CliExecutable,
    cwd: PathBuf,
    spawn_args: Vec<OsString>,
    log_path: PathBuf,
    log_file: std::fs::File,
}

/// Check auth, resolve the binary, and materialize argv plus the log file.
///
/// Every failure is already user-facing text; the caller turns it into
/// [`crate::subagent::RunState::Error`].
async fn prepare_launch<P: CliSessionPolicy>(
    request: &mut CliSessionRequest,
    policy: &mut P,
    sink: &mut StatusSink,
) -> Result<Launch, String> {
    policy.preflight(sink).await?;

    let executable = match request.overrides.executable.take() {
        Some(executable) => executable,
        None => policy.resolve_executable()?,
    };

    let frozen = request.overrides.frozen_argv.take();
    let (spawn_args, cwd) = policy.spawn_args(&request.output_file, frozen)?;

    let log_path = policy.log_path(&request.output_file);
    let program = policy.label().program;
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .map_err(|error| format!("{program}: could not open log file: {error}"))?
        .into_std()
        .await;

    Ok(Launch {
        executable,
        cwd,
        spawn_args,
        log_path,
        log_file,
    })
}

/// Spawn the child and drain it, leaving no live process tree behind.
async fn run_child<P: CliSessionPolicy>(
    request: &mut CliSessionRequest,
    policy: &mut P,
    sink: &mut StatusSink,
    launch: Launch,
) -> SessionOutcome {
    let Launch {
        executable,
        cwd,
        spawn_args,
        log_path,
        log_file,
    } = launch;
    let program = policy.label().program;

    // Typed fallible builder: Windows shim validation becomes RunState::Error
    // before spawn instead of a generic I/O failure at CreateProcess.
    let mut command = match executable.try_command(&spawn_args) {
        Ok(command) => command,
        Err(error) => return SessionOutcome::Failed(format!("{program}: {error}")),
    };
    command
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log_file)
        .kill_on_drop(true);

    if let Some(before_spawn) = request.overrides.before_spawn.as_ref() {
        if let Err(error) = before_spawn(&mut command) {
            return SessionOutcome::Failed(format!(
                "{program}: frozen executable changed before spawn: {error}"
            ));
        }
    }

    let mut child = match OwnedChild::spawn(command) {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SessionOutcome::Failed(format!("{program}: binary not found on PATH"));
        }
        Err(error) => {
            return SessionOutcome::Failed(format!(
                "{program}: failed to spawn `{}`: {error}",
                executable.display()
            ));
        }
    };

    let outcome = drain_child(request, policy, sink, &mut child, &log_path).await;
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
async fn drain_child<P: CliSessionPolicy>(
    request: &mut CliSessionRequest,
    policy: &mut P,
    sink: &mut StatusSink,
    child: &mut OwnedChild,
    log_path: &Path,
) -> SessionOutcome {
    sink.mark_running();
    let program = policy.label().program;
    let input = policy.drain_input();

    let drained = {
        let mut mapper = policy.mapper();
        let mut on_effect = |effect| sink.apply_effect(effect);
        drain::drain_child(
            child,
            drain::DrainConfig {
                program_label: program,
                max_line_bytes: MAX_NDJSON_LINE_BYTES,
            },
            &mut mapper,
            input,
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
            SessionOutcome::Failed(format!("{program}: failed waiting for child: {error}"))
        }
    }
}
