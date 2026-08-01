//! Exact, bounded process execution for frozen workflow commands.

use std::{
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use rho_sdk::{
    model::ToolSpec,
    tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolError, ToolErrorKind, ToolInvocation,
        ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, ProcessExecution,
};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{prepare_child_command, ProcessTree};

// Receipt and its exact timing command: workflow/fixtures/limit_receipt.json.
// scripts/measure_workflow_limits.py checks the recorded margin arithmetic.
const FINAL_PROCESS_CLEANUP_MILLIS: u64 = 2_000;
const HOST_CANCELLATION_COMPLETION_MILLIS: u64 = 2_500;

/// Typed termination of one exact process invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExactProcessExit {
    Code(i32),
    Signal(i32),
    Timeout,
    Cancellation,
    Abnormal,
}

/// Separately bounded byte streams from one process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExactProcessOutput {
    pub(crate) exit: ExactProcessExit,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) stdout_observed_bytes: u64,
    pub(crate) stderr_observed_bytes: u64,
    pub(crate) cleanup_incomplete: bool,
}

/// Internal SDK tool used to authorize and execute a frozen workflow command.
///
/// The caller registers one instance in a `rho_sdk::ToolHost`. ToolHost owns
/// policy, hook, approval, and execution order. This tool only declares the
/// exact process capability and runs it after that capability is authorized.
#[derive(Clone)]
pub(crate) struct WorkflowCommandTool {
    execution: ProcessExecution,
    executable_identity: crate::workflow::ExecutableIdentity,
    cwd_identity: crate::workflow::FrozenPathIdentity,
    result: Arc<Mutex<Option<ExactProcessOutput>>>,
}

impl WorkflowCommandTool {
    pub(crate) fn new(
        execution: ProcessExecution,
        executable_identity: crate::workflow::ExecutableIdentity,
        cwd_identity: crate::workflow::FrozenPathIdentity,
    ) -> Self {
        Self {
            execution,
            executable_identity,
            cwd_identity,
            result: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn take_result(&self) -> Option<ExactProcessOutput> {
        self.result
            .lock()
            .expect("workflow command result lock")
            .take()
    }
}

impl Tool for WorkflowCommandTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "workflow_command".into(),
            description: "Execute one exact command from an approved frozen workflow plan.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Process])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            if invocation
                .arguments()
                .as_object()
                .is_none_or(|value| !value.is_empty())
            {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "workflow_command arguments must be an empty object",
                ));
            }
            let execution = self.execution.clone();
            let executable_identity = self.executable_identity.clone();
            let cwd_identity = self.cwd_identity.clone();
            let result = Arc::clone(&self.result);
            let authorized_execution = authorization_execution(&execution, &executable_identity);
            let capability = CapabilityRequest::process(
                authorized_execution.clone(),
                CapabilitySource::built_in_tool("workflow_command"),
            );
            let metadata = ToolMetadata::new()
                .operation(OperationKind::Execute)
                .command_summary(format!(
                    "{} ({} arguments)",
                    authorized_execution
                        .invocation()
                        .executable_path()
                        .display(),
                    authorized_execution.invocation().arguments().len()
                ));
            Ok(
                PreparedToolInvocation::resource_aware(
                    [],
                    [capability],
                    metadata,
                    move |context| {
                        Box::pin(async move {
                            let output = run_exact_process(
                                execution,
                                &executable_identity,
                                &cwd_identity,
                                context.cancellation(),
                            )
                            .await?;
                            *result.lock().expect("workflow command result lock") = Some(output);
                            Ok(ToolOutput::text(""))
                        })
                    },
                )
                .complete_after_cancellation(Duration::from_millis(
                    HOST_CANCELLATION_COMPLETION_MILLIS,
                )),
            )
        })
    }
}

fn authorization_execution(
    execution: &ProcessExecution,
    identity: &crate::workflow::ExecutableIdentity,
) -> ProcessExecution {
    let Some(interpreter) = &identity.interpreter else {
        return execution.clone();
    };
    let mut arguments = identity.interpreter_arguments.clone();
    arguments.push(identity.file.canonical_path.clone());
    arguments.extend(execution.invocation().arguments().iter().cloned());
    if let Some(command) = execution.invocation().shell_command() {
        arguments.push(command.to_owned());
    }
    ProcessExecution::new(
        execution.working_directory(),
        rho_sdk::ProcessInvocation::executable(&interpreter.canonical_path, arguments),
        execution.environment().clone(),
        execution.output_limits(),
    )
}

async fn run_exact_process(
    execution: ProcessExecution,
    executable_identity: &crate::workflow::ExecutableIdentity,
    cwd_identity: &crate::workflow::FrozenPathIdentity,
    cancellation: &rho_sdk::CancellationToken,
) -> Result<ExactProcessOutput, ToolError> {
    ensure_handle_based_launch_supported()?;
    // This check runs after ToolHost authorization and directly before spawn.
    let verified_executable = crate::workflow::verify_executable_identity(executable_identity)
        .map_err(|error| execution_error(error.to_string()))?;
    let verified_cwd = crate::workflow::verify_directory_identity(cwd_identity)
        .map_err(|error| execution_error(error.to_string()))?;
    let mut command = command_from_execution(&execution, &verified_executable)?;
    command
        .current_dir(
            crate::workflow::verified_handle_path(
                &verified_cwd.file,
                execution.working_directory(),
            )
            .map_err(|error| execution_error(error.to_string()))?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    prepare_child_command(&mut command);
    rho_tools::apply_process_environment(&mut command, execution.environment())
        .map_err(execution_error)?;

    let mut inherited_files = vec![&verified_executable.executable.file, &verified_cwd.file];
    if let Some(interpreter) = &verified_executable.interpreter {
        inherited_files.push(&interpreter.file);
    }
    crate::workflow::configure_handle_inheritance(&mut command, &inherited_files)
        .map_err(|error| execution_error(error.to_string()))?;
    let mut child = command
        .spawn()
        .map_err(|error| execution_error(error.to_string()))?;
    let tree = ProcessTree::attach(&child).map_err(execution_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| execution_error("stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| execution_error("stderr was not captured"))?;
    let limit = execution.output_limits().max_output_bytes();
    let stdout_state = Arc::new(Mutex::new(BoundedReadState::new(limit)));
    let stderr_state = Arc::new(Mutex::new(BoundedReadState::new(limit)));
    let mut stdout_task = tokio::spawn(read_bounded(stdout, limit, Arc::clone(&stdout_state)));
    let mut stderr_task = tokio::spawn(read_bounded(stderr, limit, Arc::clone(&stderr_state)));

    let exited = child.try_wait();
    let exit = if let Some(status) = exited.transpose() {
        map_status(status)
    } else if let Some(timeout) = execution.output_limits().timeout() {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                tree.kill();
                ExactProcessExit::Cancellation
            }
            () = &mut deadline => {
                tree.kill();
                ExactProcessExit::Timeout
            }
            status = child.wait() => map_status(status),
        }
    } else {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                tree.kill();
                ExactProcessExit::Cancellation
            }
            status = child.wait() => map_status(status),
        }
    };
    // The leader may exit while descendants hold pipes. End the whole group so
    // stream drains always finish and no descendant survives the tool call.
    tree.kill();
    let cleanup = async {
        let _ = child.wait().await;
        join_reader(&mut stdout_task).await?;
        join_reader(&mut stderr_task).await
    };
    let cleanup_timed_out =
        match tokio::time::timeout(Duration::from_millis(FINAL_PROCESS_CLEANUP_MILLIS), cleanup)
            .await
        {
            Ok(result) => {
                result?;
                false
            }
            Err(_) => true,
        };
    if cleanup_timed_out {
        tree.kill();
        let _ = child.start_kill();
        stdout_task.abort();
        stderr_task.abort();
    }
    let (stdout, stdout_truncated, stdout_observed_bytes) = read_snapshot(&stdout_state);
    let (stderr, stderr_truncated, stderr_observed_bytes) = read_snapshot(&stderr_state);
    Ok(ExactProcessOutput {
        exit,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        stdout_observed_bytes,
        stderr_observed_bytes,
        cleanup_incomplete: cleanup_timed_out,
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn ensure_handle_based_launch_supported() -> Result<(), ToolError> {
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn ensure_handle_based_launch_supported() -> Result<(), ToolError> {
    Err(execution_error(
        "frozen workflow process execution is unavailable on this platform because the OS adapter cannot launch the executable, interpreter, and working directory from verified handles",
    ))
}

fn command_from_execution(
    execution: &ProcessExecution,
    verified: &crate::workflow::VerifiedExecutable,
) -> Result<tokio::process::Command, ToolError> {
    let invocation = execution.invocation();
    let executable = crate::workflow::verified_handle_path(
        &verified.executable.file,
        invocation.executable_path(),
    )
    .map_err(|error| execution_error(error.to_string()))?;
    let mut command = if let Some(interpreter) = &verified.interpreter {
        let mut command = tokio::process::Command::new(
            crate::workflow::verified_handle_path(
                &interpreter.file,
                Path::new(&interpreter.identity.canonical_path),
            )
            .map_err(|error| execution_error(error.to_string()))?,
        );
        command.args(&verified.interpreter_arguments);
        command.arg(executable);
        command
    } else {
        tokio::process::Command::new(executable)
    };
    command.args(invocation.arguments());
    if let Some(shell_command) = invocation.shell_command() {
        command.arg(shell_command);
    }
    Ok(command)
}

async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
    state: Arc<Mutex<BoundedReadState>>,
) -> std::io::Result<()> {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let mut state = state.lock().expect("bounded workflow stream lock");
        state.observed_bytes = state.observed_bytes.saturating_add(read as u64);
        let remaining = limit.saturating_sub(state.retained.len());
        let keep = read.min(remaining);
        state.retained.extend_from_slice(&buffer[..keep]);
        state.truncated |= keep < read;
    }
    Ok(())
}

struct BoundedReadState {
    retained: Vec<u8>,
    truncated: bool,
    observed_bytes: u64,
}

impl BoundedReadState {
    fn new(limit: usize) -> Self {
        Self {
            retained: Vec::with_capacity(limit.min(8 * 1024)),
            truncated: false,
            observed_bytes: 0,
        }
    }
}

fn read_snapshot(state: &Mutex<BoundedReadState>) -> (Vec<u8>, bool, u64) {
    let state = state.lock().expect("bounded workflow stream lock");
    (
        state.retained.clone(),
        state.truncated,
        state.observed_bytes,
    )
}

async fn join_reader(
    task: &mut tokio::task::JoinHandle<std::io::Result<()>>,
) -> Result<(), ToolError> {
    task.await
        .map_err(|error| execution_error(format!("output reader failed: {error}")))?
        .map_err(|error| execution_error(format!("output read failed: {error}")))
}

fn map_status(status: std::io::Result<ExitStatus>) -> ExactProcessExit {
    let Ok(status) = status else {
        return ExactProcessExit::Abnormal;
    };
    if let Some(code) = status.code() {
        return ExactProcessExit::Code(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ExactProcessExit::Signal(signal);
        }
    }
    ExactProcessExit::Abnormal
}

fn execution_error(message: impl Into<String>) -> ToolError {
    ToolError::new(ToolErrorKind::Execution, message)
}

#[cfg(test)]
#[path = "exact_tests.rs"]
mod tests;
