use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use crate::{
    app::agent_executor::{AgentExecutor, FrozenAgentLaunchRequest},
    subagent::RunState,
    workflow::{
        ArtifactObservation, NodeExecution, NodeTerminalState, ResolvedNode, ValidatedOutputRef,
        WorkflowValue,
    },
};

use super::{
    artifacts::{write_artifact, write_artifact_with_observation},
    cancellation::AGENT_CANCELLATION_CLEANUP_MILLIS,
    command::render_template,
    CleanupCause, NodeExecutionRequest, NodeExecutionResult, RuntimeError, WorkflowExecutionFuture,
    WorkflowNodeExecutor,
};

pub(crate) struct WorkflowAgentExecutor {
    executor: Arc<AgentExecutor>,
}

impl WorkflowAgentExecutor {
    pub(crate) fn new(executor: Arc<AgentExecutor>) -> Self {
        Self { executor }
    }
}

impl WorkflowNodeExecutor for WorkflowAgentExecutor {
    fn execute<'a>(&'a self, request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a> {
        Box::pin(async move { self.execute_agent(request).await })
    }
}

impl WorkflowAgentExecutor {
    async fn execute_agent(
        &self,
        request: NodeExecutionRequest,
    ) -> Result<NodeExecutionResult, RuntimeError> {
        let node = &request.workflow.graph.nodes[&request.node];
        let NodeExecution::Agent(agent_node) = &node.execution else {
            return Err(RuntimeError::LaunchMetadata { node: request.node });
        };
        let Some(ResolvedNode::Agent(agent)) = request.workflow.resolved_nodes.get(&request.node)
        else {
            return Err(RuntimeError::LaunchMetadata { node: request.node });
        };
        let mut prompt = render_template(
            &agent_node.prompt,
            &request.outputs,
            &request.workflow.runtime_limits,
        )?;
        if let Some(schema) = &agent_node.output {
            let normalized = serde_json::to_string(schema)?;
            prompt.push_str(
                "\n\nReturn exactly one JSON value as the final answer. Do not use a code fence. Schema: ",
            );
            prompt.push_str(&normalized);
        }
        super::command::check_runtime_limit(
            "prompt expansion bytes",
            request.workflow.runtime_limits.prompt_expansion_bytes,
            prompt.len() as u64,
        )?;
        let agent_directory = request.attempt_directory.join("agent");
        let run_directory = request
            .attempt_directory
            .ancestors()
            .nth(4)
            .ok_or_else(|| RuntimeError::UnsafeArtifact(request.attempt_directory.clone()))?;
        crate::workflow::ensure_directory_beneath(
            run_directory,
            agent_directory
                .strip_prefix(run_directory)
                .map_err(|_| RuntimeError::UnsafeArtifact(agent_directory.clone()))?,
        )?;
        let output_file = agent_directory.join(crate::subagent::RESULT_FILE_NAME);
        let mut handle = self
            .executor
            .spawn_frozen(FrozenAgentLaunchRequest {
                agent: agent.as_ref().clone(),
                prompt,
                run_id: format!(
                    "workflow:{}:{}:{}",
                    request.run_id, request.node, request.attempt
                ),
                output_file: output_file.clone(),
                hook_host_labels: rho_sdk::hooks::HookHostLabels::new()
                    .label("workflow_run_id", request.run_id.to_string())
                    .label("plan_digest", request.workflow.graph_digest.0.clone())
                    .label("node_id", request.node.to_string())
                    .label("attempt", request.attempt.to_string()),
            })
            .map_err(|error| RuntimeError::Executor(error.to_string()))?;
        let deadline = tokio::time::sleep(Duration::from_secs(node.timeout_seconds));
        tokio::pin!(deadline);
        let status = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                return stop_agent(
                    &mut handle,
                    AgentStopReason::Cancellation,
                    agent_cleanup_limit(),
                ).await;
            }
            () = &mut deadline => {
                return stop_agent(
                    &mut handle,
                    AgentStopReason::Timeout,
                    agent_cleanup_limit(),
                ).await;
            }
            status = handle.wait() => status,
        };
        let outcome = match status.state {
            RunState::Ok => NodeTerminalState::Success,
            RunState::Stopped => NodeTerminalState::Cancellation,
            RunState::Error | RunState::Starting | RunState::Running => NodeTerminalState::Failure,
        };
        let mut result = NodeExecutionResult::terminal(outcome);
        let Some(answer) = status.result else {
            if agent_node.output.is_some() {
                result.outcome = NodeTerminalState::Failure;
            }
            return Ok(result);
        };
        let max_output_bytes = usize::try_from(node.max_output_bytes).map_err(|_| {
            RuntimeError::Data(format!(
                "node '{}' output limit does not fit this platform",
                request.node
            ))
        })?;
        let mut retained = answer.len().min(max_output_bytes);
        while !answer.is_char_boundary(retained) {
            retained -= 1;
        }
        let answer_truncated = retained < answer.len();
        let answer_artifact = write_artifact_with_observation(
            run_directory,
            &agent_directory.join("answer.txt"),
            &answer.as_bytes()[..retained],
            if answer_truncated {
                ArtifactObservation::Truncated {
                    observed_bytes_at_least: answer.len() as u64,
                }
            } else {
                ArtifactObservation::Complete {
                    observed_bytes: answer.len() as u64,
                }
            },
        )?;
        result.artifacts.answer = Some(answer_artifact);
        if let Some(schema) = &agent_node.output {
            if answer_truncated {
                result.outcome = NodeTerminalState::Failure;
                return Ok(result);
            }
            let parsed = serde_json::from_str(&answer)
                .map_err(RuntimeError::from)
                .and_then(|json| WorkflowValue::from_json(json).map_err(RuntimeError::from))
                .and_then(|value| {
                    schema.validate_value(&value)?;
                    Ok(value)
                });
            match parsed {
                Ok(value) => {
                    let artifact = write_artifact(
                        run_directory,
                        &request.attempt_directory.join("output.json"),
                        &serde_json::to_vec_pretty(&value)?,
                    )?;
                    result.artifacts.structured_output = Some(artifact.clone());
                    result.structured_output = Some(ValidatedOutputRef { artifact, value });
                }
                Err(_) => result.outcome = NodeTerminalState::Failure,
            }
        }
        Ok(result)
    }
}

fn agent_cleanup_limit() -> Duration {
    Duration::from_millis(AGENT_CANCELLATION_CLEANUP_MILLIS)
}

pub(super) type AgentCleanupFuture<'a> =
    Pin<Box<dyn Future<Output = crate::subagent::RunStatus> + Send + 'a>>;

/// A cancelled agent execution whose terminal state can be confirmed.
pub(super) trait AgentCleanupHandle {
    fn cancel(&self);
    fn wait(&mut self) -> AgentCleanupFuture<'_>;
}

impl AgentCleanupHandle for crate::app::agent_executor::AgentRunHandle {
    fn cancel(&self) {
        crate::app::agent_executor::AgentRunHandle::cancel(self);
    }

    fn wait(&mut self) -> AgentCleanupFuture<'_> {
        Box::pin(crate::app::agent_executor::AgentRunHandle::wait(self))
    }
}

#[derive(Clone, Copy)]
pub(super) enum AgentStopReason {
    Cancellation,
    Timeout,
}

pub(super) async fn stop_agent(
    handle: &mut impl AgentCleanupHandle,
    reason: AgentStopReason,
    limit: Duration,
) -> Result<NodeExecutionResult, RuntimeError> {
    handle.cancel();
    match tokio::time::timeout(limit, handle.wait()).await {
        Ok(status) if status.state.is_terminal() => {
            Ok(NodeExecutionResult::terminal(match reason {
                AgentStopReason::Cancellation => NodeTerminalState::Cancellation,
                AgentStopReason::Timeout => NodeTerminalState::Failure,
            }))
        }
        Ok(_) | Err(_) => Err(RuntimeError::CleanupUncertain {
            cause: match reason {
                AgentStopReason::Cancellation => CleanupCause::Cancellation,
                AgentStopReason::Timeout => CleanupCause::Timeout,
            },
        }),
    }
}
