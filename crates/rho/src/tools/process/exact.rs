//! Exact, bounded process execution for frozen workflow commands.

use std::{
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
}

/// Internal SDK tool used to authorize and execute a frozen workflow command.
///
/// The caller registers one instance in a `rho_sdk::ToolHost`. ToolHost owns
/// policy, hook, approval, and execution order. This tool only declares the
/// exact process capability and runs it after that capability is authorized.
#[derive(Clone)]
pub(crate) struct WorkflowCommandTool {
    execution: ProcessExecution,
    result: Arc<Mutex<Option<ExactProcessOutput>>>,
}

impl WorkflowCommandTool {
    pub(crate) fn new(execution: ProcessExecution) -> Self {
        Self {
            execution,
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
            let result = Arc::clone(&self.result);
            let capability = CapabilityRequest::process(
                execution.clone(),
                CapabilitySource::built_in_tool("workflow_command"),
            );
            let metadata = ToolMetadata::new()
                .operation(OperationKind::Execute)
                .command_summary(format!(
                    "{} ({} arguments)",
                    execution.invocation().executable_path().display(),
                    execution.invocation().arguments().len()
                ));
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [capability],
                metadata,
                move |context| {
                    Box::pin(async move {
                        let output = run_exact_process(execution, context.cancellation()).await?;
                        *result.lock().expect("workflow command result lock") = Some(output);
                        Ok(ToolOutput::text(""))
                    })
                },
            ))
        })
    }
}

async fn run_exact_process(
    execution: ProcessExecution,
    cancellation: &rho_sdk::CancellationToken,
) -> Result<ExactProcessOutput, ToolError> {
    let mut command = command_from_execution(&execution);
    command
        .current_dir(execution.working_directory())
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
    let stdout_task = tokio::spawn(read_bounded(stdout, limit));
    let stderr_task = tokio::spawn(read_bounded(stderr, limit));

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
    let (stdout, stdout_truncated) = join_reader(stdout_task).await?;
    let (stderr, stderr_truncated) = join_reader(stderr_task).await?;
    Ok(ExactProcessOutput {
        exit,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn command_from_execution(execution: &ProcessExecution) -> tokio::process::Command {
    let invocation = execution.invocation();
    let mut command = tokio::process::Command::new(invocation.executable_path());
    command.args(invocation.arguments());
    if let Some(shell_command) = invocation.shell_command() {
        command.arg(shell_command);
    }
    command
}

async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(8 * 1024));
    let mut truncated = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let keep = read.min(remaining);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

async fn join_reader(
    task: tokio::task::JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, bool), ToolError> {
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
