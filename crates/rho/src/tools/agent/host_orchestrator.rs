//! T3-specific transport policy. Runs belong to the host, not SubagentManager.

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use rho_sdk::{
    tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolInvocation, ToolOutput},
    ToolCallId,
};
use serde_json::{json, Value};

use crate::{
    agent::AgentDefinition,
    tools::{mcp::exported_name::namespaced_tool_name, sdk_registry::AppToolSet},
};

const SERVER: &str = "t3-code";

pub(super) struct HostOrchestrator {
    /// The bound T3 task tools, or why they could not be bound.
    contract: Result<HostContract, String>,
    runs: Mutex<Vec<HostRun>>,
}

/// T3's three task tools, bound only as a complete set.
struct HostContract {
    delegate: Arc<dyn Tool>,
    status: Arc<dyn Tool>,
    cancel: Arc<dyn Tool>,
}

impl HostContract {
    fn bind(tools: &AppToolSet) -> Result<Self, String> {
        let lookup = |name| {
            tools
                .tool_named(&namespaced_tool_name(SERVER, name))
                .ok_or_else(|| {
                    format!(
                        "--t3code needs tool `{name}` from MCP server `{SERVER}`, which \
                         did not provide it; is Rho running inside T3 Code?"
                    )
                })
        };
        Ok(Self {
            delegate: lookup("delegate_task")?,
            status: lookup("task_status")?,
            cancel: lookup("task_cancel")?,
        })
    }
}

#[derive(Clone)]
pub(super) struct HostRun {
    pub task_id: String,
    pub child_thread_id: Option<String>,
    pub agent_id: String,
    pub title: String,
    pub started: Instant,
}

impl HostOrchestrator {
    /// Binds T3's three task tools from the `t3-code` server. A missing tool
    /// does not fail the session: hosts open sessions without MCP servers too
    /// (T3's provider status probe does). Instead every host operation fails
    /// with the reason, so the user's `--t3code` opt-in never silently falls
    /// back to in-process subagents.
    pub(super) fn connect(tools: &AppToolSet) -> Self {
        Self {
            contract: HostContract::bind(tools),
            runs: Mutex::new(Vec::new()),
        }
    }

    fn contract(&self) -> Result<&HostContract, ToolError> {
        self.contract
            .as_ref()
            .map_err(|reason| ToolError::new(ToolErrorKind::Execution, reason.as_str()))
    }

    pub(super) async fn delegate(
        &self,
        invocation_id: ToolCallId,
        definition: &AgentDefinition,
        prompt: &str,
        context: ToolContext,
    ) -> Result<HostRun, ToolError> {
        let arguments = delegate_arguments(definition, prompt);
        let output = self
            .contract()?
            .delegate
            .call(ToolInvocation::new(invocation_id, arguments), context)
            .await
            .map_err(|error| {
                if error.kind() == ToolErrorKind::Cancelled {
                    error
                } else {
                    ToolError::new(ToolErrorKind::Execution, error.message())
                }
            })?;
        if output.is_failure() {
            return Err(ToolError::new(ToolErrorKind::Execution, output.content()));
        }
        let data = output.structured_content();
        let task_id = data
            .and_then(|data| data.get("taskId"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::Execution,
                    format!(
                        "t3-code delegate_task returned no taskId: {}; structured content: {data:?}",
                        output.content()
                    ),
                )
            })?;
        let run = HostRun {
            task_id: task_id.to_owned(),
            child_thread_id: data
                .and_then(|data| data.get("childThreadId"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            agent_id: definition.id.to_string(),
            // Local run label only; T3 owns its thread's display title.
            title: prompt
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or(definition.id.as_str())
                .to_owned(),
            started: Instant::now(),
        };
        self.runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(run.clone());
        // No drop/shutdown cancellation: the host owns this child's lifetime,
        // including when cancellation races the delegate_task response.
        Ok(run)
    }

    pub(super) fn runs(&self) -> Vec<HostRun> {
        self.runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(super) fn run(&self, task_id: &str) -> Option<HostRun> {
        self.runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|run| run.task_id == task_id)
            .cloned()
    }

    /// Keep the host's response opaque: T3 owns status and completion semantics.
    pub(super) async fn status(
        &self,
        invocation_id: ToolCallId,
        task_id: &str,
        context: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        self.contract()?
            .status
            .call(
                ToolInvocation::new(invocation_id, json!({"taskId": task_id})),
                context,
            )
            .await
    }

    pub(super) async fn cancel(
        &self,
        invocation_id: ToolCallId,
        task_id: &str,
        reason: Option<&str>,
        context: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let mut arguments = json!({"taskId": task_id});
        if let Some(reason) = reason {
            arguments["reason"] = json!(reason);
        }
        self.contract()?
            .cancel
            .call(ToolInvocation::new(invocation_id, arguments), context)
            .await
    }
}

/// A child receives just the rebinding command and the self-contained task.
/// No `target.model` is sent: T3 accepts only models the ACP agent advertises,
/// which excludes most pinned definition models (measured: `openai-codex/
/// gpt-6-luna` failed with `model_unavailable`). The child's `/agent:` rebind
/// applies the definition's model policy itself.
fn delegate_arguments(definition: &AgentDefinition, prompt: &str) -> Value {
    // Let T3 derive its display title from the task. Copying instructions into
    // the separate title field can exceed its budget and reject the launch.
    json!({
        "task": format!("/agent:{} {prompt}", definition.id),
        "mode": "async",
    })
}

#[cfg(test)]
#[path = "host_orchestrator_tests.rs"]
mod tests;
