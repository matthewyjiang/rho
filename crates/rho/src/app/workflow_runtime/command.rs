use std::{path::Path, sync::Arc, time::Duration};

use rho_sdk::{
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits, ToolHost,
    ToolHostCall,
};

use crate::{
    tools::process::{ExactProcessExit, WorkflowCommandTool},
    workflow::{
        CommandExit, CommandNode, CommandOutcome, NodeExecution, NodeTerminalState, ResolvedNode,
        Template, TemplatePart, ValidatedOutputRef, WorkflowValue,
    },
};

use super::{
    artifacts::{write_artifact, write_json},
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

impl WorkflowNodeExecutor for WorkflowCommandExecutor {
    fn execute<'a>(&'a self, request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a> {
        Box::pin(async move { self.execute_command(request).await })
    }
}

impl WorkflowCommandExecutor {
    async fn execute_command(
        &self,
        request: NodeExecutionRequest,
    ) -> Result<NodeExecutionResult, RuntimeError> {
        let node = &request.workflow.graph.nodes[&request.node];
        let NodeExecution::Command(command) = &node.execution else {
            return Err(RuntimeError::LaunchMetadata { node: request.node });
        };
        let Some(ResolvedNode::Command(resolved)) =
            request.workflow.resolved_nodes.get(&request.node)
        else {
            return Err(RuntimeError::LaunchMetadata { node: request.node });
        };
        if !resolved.exact_path {
            return Err(RuntimeError::Data(format!(
                "node '{}' executable was not frozen as an exact path",
                request.node
            )));
        }
        let executable = Path::new(&resolved.executable).canonicalize()?;
        if executable != Path::new(&resolved.executable) {
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
        let invocation = invocation(command, &executable, &request.outputs)?;
        let max_output_bytes = usize::try_from(node.max_output_bytes).map_err(|_| {
            RuntimeError::Data(format!(
                "node '{}' output limit does not fit this platform",
                request.node
            ))
        })?;
        let execution = ProcessExecution::new(
            cwd,
            invocation,
            self.environment.clone(),
            ProcessOutputLimits::new(
                max_output_bytes,
                Some(Duration::from_secs(node.timeout_seconds)),
            ),
        );
        let tool = WorkflowCommandTool::new(execution);
        let labels = rho_sdk::hooks::HookHostLabels::new()
            .label("workflow_run_id", request.run_id.to_string())
            .label("plan_digest", request.workflow.graph_digest.0.clone())
            .label("node_id", request.node.to_string())
            .label("attempt", request.attempt.to_string());
        let host = self.hosts.create(tool.clone(), labels)?;
        let mut run = host
            .start(ToolHostCall::new("workflow_command", serde_json::json!({})))
            .map_err(map_host_error)?;
        let cancellation = request.cancellation.clone();
        let host_result = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                run.cancel();
                let _ = run.outcome().await;
                return Ok(NodeExecutionResult {
                    outcome: NodeTerminalState::Cancellation,
                    command_exit: Some(CommandExit::Cancellation),
                    output: None,
                });
            }
            result = run.outcome() => result,
        };
        host_result.map_err(map_host_error)?;
        let output = tool.take_result().ok_or_else(|| {
            RuntimeError::Executor("workflow_command returned without a process result".into())
        })?;
        let run_directory = request
            .attempt_directory
            .ancestors()
            .nth(4)
            .ok_or_else(|| RuntimeError::UnsafeArtifact(request.attempt_directory.clone()))?;
        let stdout = write_artifact(
            run_directory,
            &request.attempt_directory.join("stdout"),
            &output.stdout,
        )?;
        let stderr = write_artifact(
            run_directory,
            &request.attempt_directory.join("stderr"),
            &output.stderr,
        )?;
        let exit = map_exit(output.exit);
        let mut structured_output = None;
        let mut value = None;
        let mut outcome = exit_outcome(&exit);
        if let Some(schema) = command.output() {
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
                        value = Some(parsed);
                    }
                    Err(_) => outcome = NodeTerminalState::Failure,
                }
            }
        }
        let command_outcome = CommandOutcome {
            exit: exit.clone(),
            stdout,
            stderr,
            structured_output,
        };
        write_json(
            run_directory,
            &request.attempt_directory.join("command.json"),
            &command_outcome,
        )?;
        Ok(NodeExecutionResult {
            outcome,
            command_exit: Some(exit),
            output: value,
        })
    }
}

fn invocation(
    command: &CommandNode,
    executable: &Path,
    outputs: &std::collections::BTreeMap<crate::workflow::NodeId, WorkflowValue>,
) -> Result<ProcessInvocation, RuntimeError> {
    Ok(match command {
        CommandNode::Direct { arguments, .. } => ProcessInvocation::executable(
            executable,
            arguments
                .iter()
                .map(|argument| render_template(argument, outputs))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        CommandNode::Shell {
            arguments, command, ..
        } => ProcessInvocation::shell(executable, arguments.clone(), command),
    })
}

pub(super) fn render_template(
    template: &Template,
    outputs: &std::collections::BTreeMap<crate::workflow::NodeId, WorkflowValue>,
) -> Result<String, RuntimeError> {
    let mut rendered = String::new();
    for part in &template.0 {
        match part {
            TemplatePart::Literal { value } => rendered.push_str(value),
            TemplatePart::Output { reference } => {
                let value = outputs
                    .get(&reference.node)
                    .and_then(|value| value.at_path(&reference.path.0))
                    .ok_or_else(|| {
                        RuntimeError::Data(format!(
                            "required output '{}.{}' is unavailable",
                            reference.node,
                            reference.path.0.join(".")
                        ))
                    })?;
                rendered.push_str(&value.to_string());
            }
        }
    }
    Ok(rendered)
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
