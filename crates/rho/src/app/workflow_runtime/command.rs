use std::{path::Path, sync::Arc, time::Duration};

use rho_sdk::{ProcessEnvironment, ProcessExecution, ProcessOutputLimits, ToolHost, ToolHostCall};

use crate::{
    tools::process::{ExactProcessExit, WorkflowCommandTool},
    workflow::{
        ArtifactObservation, AttemptArtifacts, CommandExit, CommandOutcome, NodeTerminalState,
        ValidatedOutputRef, WorkflowValue,
    },
};

use super::{
    artifacts::{write_artifact, write_artifact_with_observation, write_json},
    prepared::CommandInvocation,
    NodeExecutionRequest, NodeExecutionResult, RuntimeError, WorkflowExecutionFuture,
    WorkflowNodeExecutor,
};

/// Composition seam that registers each exact command tool in a configured
/// SDK ToolHost. The host supplied here owns current policy, hooks, and approval.
pub(crate) trait CommandHostFactory: Send + Sync {
    fn create(
        &self,
        tool: WorkflowCommandTool,
        labels: rho_sdk::hooks::HookHostLabels,
    ) -> Result<ToolHost, RuntimeError>;
}

pub(crate) struct WorkflowCommandExecutor {
    environment: ProcessEnvironment,
    hosts: Arc<dyn CommandHostFactory>,
}

impl WorkflowCommandExecutor {
    pub(crate) fn new(environment: ProcessEnvironment, hosts: Arc<dyn CommandHostFactory>) -> Self {
        Self { environment, hosts }
    }
}

impl WorkflowNodeExecutor<CommandInvocation> for WorkflowCommandExecutor {
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest<CommandInvocation>,
    ) -> WorkflowExecutionFuture<'a> {
        Box::pin(async move { self.execute_command(request).await })
    }
}

impl WorkflowCommandExecutor {
    async fn execute_command(
        &self,
        request: NodeExecutionRequest<CommandInvocation>,
    ) -> Result<NodeExecutionResult, RuntimeError> {
        let prepared = &request.invocation;
        let CommandInvocation {
            resolved,
            invocation,
        } = &prepared.execution;
        if !resolved.exact_path {
            return Err(RuntimeError::Data(format!(
                "node '{}' executable was not frozen as an exact path",
                request.node
            )));
        }
        let executable = Path::new(&resolved.executable).canonicalize()?;
        if !canonical_paths_match(&executable, Path::new(&resolved.executable)) {
            return Err(RuntimeError::Data(format!(
                "node '{}' executable path is not canonical",
                request.node
            )));
        }
        let cwd = Path::new(&resolved.cwd).canonicalize()?;
        let workspace = request.workspace.canonicalize()?;
        if !cwd.starts_with(&workspace) {
            return Err(RuntimeError::Data(format!(
                "node '{}' working directory is outside the workspace",
                request.node
            )));
        }
        if let Some(progress) = &request.progress {
            progress.message(command_progress_message(&executable, invocation));
        }
        let max_output_bytes = usize::try_from(prepared.max_output_bytes).map_err(|_| {
            RuntimeError::Data(format!(
                "node '{}' output limit does not fit this platform",
                request.node
            ))
        })?;
        let execution = ProcessExecution::new(
            cwd,
            invocation.clone(),
            self.environment.clone(),
            ProcessOutputLimits::new(
                max_output_bytes,
                Some(Duration::from_secs(prepared.timeout_seconds)),
            ),
        );
        let tool = WorkflowCommandTool::new(
            execution,
            resolved.executable_identity.clone(),
            resolved.cwd_identity.clone(),
        );
        let labels = rho_sdk::hooks::HookHostLabels::new()
            .label("workflow_run_id", request.run_id.to_string())
            .label("plan_digest", request.plan_digest.0.clone())
            .label("node_id", request.node.to_string())
            .label("attempt", request.attempt.to_string());
        let host = self.hosts.create(tool.clone(), labels)?;
        let mut run = host
            .start(ToolHostCall::new("workflow_command", serde_json::json!({})))
            .map_err(map_host_error)?;
        let host_cancellation = run.cancellation_handle();
        let mut host_outcome = Box::pin(run.outcome());
        let cancellation = request.cancellation.clone();
        let host_result = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                host_cancellation.cancel();
                host_outcome.await
            }
            result = &mut host_outcome => result,
        };
        let output = match tool.take_result() {
            Some(output) => output,
            None => {
                host_result.map_err(map_host_error)?;
                return Err(RuntimeError::Executor(
                    "workflow_command returned without a process result".into(),
                ));
            }
        };
        let run_directory = &request.run_directory;
        let stdout = write_artifact_with_observation(
            run_directory,
            &request.attempt_directory.join("stdout"),
            &output.stdout,
            stream_observation(
                output.stdout_observed_bytes,
                output.stdout_truncated,
                output.cleanup_incomplete,
            ),
        )?;
        let stderr = write_artifact_with_observation(
            run_directory,
            &request.attempt_directory.join("stderr"),
            &output.stderr,
            stream_observation(
                output.stderr_observed_bytes,
                output.stderr_truncated,
                output.cleanup_incomplete,
            ),
        )?;
        let exit = map_exit(output.exit);
        let mut structured_output = None;
        let mut outcome = process_outcome(&exit, output.cleanup_incomplete);
        let successful_schema = (outcome == NodeTerminalState::Success)
            .then_some(prepared.output.as_ref())
            .flatten();
        if let Some(schema) = successful_schema {
            if output.stdout_truncated {
                outcome = NodeTerminalState::Failure;
            } else {
                match serde_json::from_slice(&output.stdout)
                    .map_err(RuntimeError::from)
                    .and_then(|json| WorkflowValue::from_json(json).map_err(RuntimeError::from))
                    .and_then(|parsed| {
                        schema.validate_value(&parsed)?;
                        Ok(parsed)
                    }) {
                    Ok(parsed) => {
                        let artifact = write_artifact(
                            run_directory,
                            &request.attempt_directory.join("output.json"),
                            &serde_json::to_vec_pretty(&parsed)?,
                        )?;
                        structured_output = Some(ValidatedOutputRef {
                            artifact,
                            value: parsed.clone(),
                        });
                    }
                    Err(error) => {
                        if let Some(progress) = &request.progress {
                            progress.message(error.to_string());
                        }
                        outcome = NodeTerminalState::Failure;
                    }
                }
            }
        }
        let command_outcome = CommandOutcome {
            exit: exit.clone(),
            stdout,
            stderr,
            structured_output,
        };
        let command_artifact = write_json(
            run_directory,
            &request.attempt_directory.join("command.json"),
            &command_outcome,
        )?;
        Ok(NodeExecutionResult {
            outcome,
            command_exit: Some(exit),
            structured_output: command_outcome.structured_output.clone(),
            artifacts: AttemptArtifacts {
                stdout: Some(command_outcome.stdout),
                stderr: Some(command_outcome.stderr),
                answer: None,
                structured_output: command_outcome
                    .structured_output
                    .as_ref()
                    .map(|output| output.artifact.clone()),
                command_outcome: Some(command_artifact),
            },
        })
    }
}

#[cfg(not(windows))]
fn canonical_paths_match(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(windows)]
fn canonical_paths_match(left: &Path, right: &Path) -> bool {
    crate::workflow::windows_paths_match(left, right)
}

fn stream_observation(
    observed_bytes: u64,
    truncated: bool,
    cleanup_incomplete: bool,
) -> ArtifactObservation {
    if cleanup_incomplete {
        ArtifactObservation::Incomplete { observed_bytes }
    } else if truncated {
        ArtifactObservation::Truncated {
            observed_bytes_at_least: observed_bytes,
        }
    } else {
        ArtifactObservation::Complete { observed_bytes }
    }
}

fn map_exit(exit: ExactProcessExit) -> CommandExit {
    match exit {
        ExactProcessExit::Code(code) => CommandExit::Code { code },
        ExactProcessExit::Signal(signal) => CommandExit::Signal { signal },
        ExactProcessExit::Timeout => CommandExit::Timeout,
        ExactProcessExit::Cancellation => CommandExit::Cancellation,
        ExactProcessExit::Abnormal => CommandExit::Abnormal,
    }
}

fn exit_outcome(exit: &CommandExit) -> NodeTerminalState {
    match exit {
        CommandExit::Code { code: 0 } => NodeTerminalState::Success,
        CommandExit::Cancellation => NodeTerminalState::Cancellation,
        CommandExit::Code { .. }
        | CommandExit::Signal { .. }
        | CommandExit::Timeout
        | CommandExit::Abnormal => NodeTerminalState::Failure,
    }
}

fn process_outcome(exit: &CommandExit, cleanup_incomplete: bool) -> NodeTerminalState {
    let outcome = exit_outcome(exit);
    if cleanup_incomplete && outcome == NodeTerminalState::Success {
        NodeTerminalState::Failure
    } else {
        outcome
    }
}

fn map_host_error(error: rho_sdk::Error) -> RuntimeError {
    match error {
        rho_sdk::Error::Tool(error)
            if error.kind() == rho_sdk::tool::ToolErrorKind::PolicyDenied =>
        {
            RuntimeError::Denied(error.message().to_owned())
        }
        rho_sdk::Error::Tool(error) if error.kind() == rho_sdk::tool::ToolErrorKind::Cancelled => {
            RuntimeError::Cancelled
        }
        error => RuntimeError::Executor(error.to_string()),
    }
}

fn command_progress_message(executable: &Path, invocation: &rho_sdk::ProcessInvocation) -> String {
    match invocation.shell_command() {
        Some(command) => {
            let shell = executable
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("shell");
            format!("running {shell}: {command}")
        }
        None => {
            let exe = executable
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("command");
            let args = invocation.arguments();
            if args.is_empty() {
                format!("running {exe}")
            } else {
                // Preserve the existing compact activity-preview width.
                let summary = super::agent::truncate_chars(&args.join(" "), 140);
                format!("running {exe} {summary}")
            }
        }
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
