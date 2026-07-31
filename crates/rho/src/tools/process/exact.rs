//! Exact, bounded process execution for frozen workflow commands.

use std::{
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
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
            Ok(PreparedToolInvocation::resource_aware(
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
            ))
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
    let total_limit = limit.saturating_mul(2);
    let total_remaining = Arc::new(AtomicUsize::new(total_limit));
    let stdout_task = tokio::spawn(read_bounded(stdout, limit, Arc::clone(&total_remaining)));
    let stderr_task = tokio::spawn(read_bounded(stderr, limit, total_remaining));

    let exited = child.try_wait();
    let exit = if let Some(status) = exited.transpose() {
        map_status(status)
    } else if let Some(timeout) = execution.output_limits().timeout() {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                tree.terminate(&mut child, Duration::ZERO).await;
                ExactProcessExit::Cancellation
            }
            () = &mut deadline => {
                tree.terminate(&mut child, Duration::ZERO).await;
                ExactProcessExit::Timeout
            }
            status = child.wait() => map_status(status),
        }
    } else {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                tree.terminate(&mut child, Duration::ZERO).await;
                ExactProcessExit::Cancellation
            }
            status = child.wait() => map_status(status),
        }
    };
    // The leader may exit while descendants hold pipes. End the whole group so
    // stream drains always finish and no descendant survives the tool call.
    tree.kill();
    let (stdout, stdout_truncated, stdout_observed_bytes) = join_reader(stdout_task).await?;
    let (stderr, stderr_truncated, stderr_observed_bytes) = join_reader(stderr_task).await?;
    Ok(ExactProcessOutput {
        exit,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        stdout_observed_bytes,
        stderr_observed_bytes,
    })
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
    total_remaining: Arc<AtomicUsize>,
) -> std::io::Result<(Vec<u8>, bool, u64)> {
    let mut retained = Vec::with_capacity(limit.min(8 * 1024));
    let mut truncated = false;
    let mut observed_bytes = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        observed_bytes = observed_bytes.saturating_add(read as u64);
        let remaining = limit.saturating_sub(retained.len());
        let wanted = read.min(remaining);
        let keep = total_remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |total| {
                Some(total.saturating_sub(wanted))
            })
            .unwrap_or(0)
            .min(wanted);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated, observed_bytes))
}

async fn join_reader(
    task: tokio::task::JoinHandle<std::io::Result<(Vec<u8>, bool, u64)>>,
) -> Result<(Vec<u8>, bool, u64), ToolError> {
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
