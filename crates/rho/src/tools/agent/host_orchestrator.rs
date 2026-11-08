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

/// T3's three task tools, bound only as a complete set. The session's
/// host-backed `agent` tools delegate its children through it, and the
/// daemon's T3 wake watchers (`daemon_host::sessions::t3_wake`) start and
/// cancel their watcher tasks through it.
pub(crate) struct HostContract {
    delegate: Arc<dyn Tool>,
    status: Arc<dyn Tool>,
    cancel: Arc<dyn Tool>,
}

/// A task T3 accepted.
pub(crate) struct DelegatedTask {
    pub(crate) task_id: String,
    /// The thread T3 runs the task in, when it said.
    pub(crate) child_thread_id: Option<String>,
}

impl HostContract {
    /// Binds the task tools of the `t3-code` server that `lookup` finds by
    /// exported name, or says which one is missing.
    pub(crate) fn bind(lookup: impl Fn(&str) -> Option<Arc<dyn Tool>>) -> Result<Self, String> {
        let lookup = |name| {
            lookup(&namespaced_tool_name(SERVER, name)).ok_or_else(|| {
                format!(
                    "T3 Code task tool `{name}` is missing from MCP server `{SERVER}`; \
                     is Rho running inside T3 Code?"
                )
            })
        };
        Ok(Self {
            delegate: lookup("delegate_task")?,
            status: lookup("task_status")?,
            cancel: lookup("task_cancel")?,
        })
    }

    /// Starts a T3 task with `invocation`'s arguments. A refusal, or a reply
    /// without a task id, is an execution error; a cancel stays a cancel.
    pub(crate) async fn delegate(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<DelegatedTask, ToolError> {
        let output = self
            .delegate
            .call(invocation, context)
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
        Ok(DelegatedTask {
            task_id: task_id.to_owned(),
            child_thread_id: data
                .and_then(|data| data.get("childThreadId"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    /// Asks T3 to cancel task `task_id`, for `reason` when given. T3's
    /// reply stays opaque.
    pub(crate) async fn cancel(
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
        self.cancel
            .call(ToolInvocation::new(invocation_id, arguments), context)
            .await
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
    /// with the reason, so the host-threads opt-in never silently falls back to
    /// in-process subagents.
    pub(super) fn connect(tools: &AppToolSet) -> Self {
        Self {
            contract: HostContract::bind(|name| tools.tool_named(name)),
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
        model_override: Option<&str>,
        context: ToolContext,
    ) -> Result<HostRun, ToolError> {
        let arguments = delegate_arguments(definition, prompt, model_override);
        let DelegatedTask {
            task_id,
            child_thread_id,
        } = self
            .contract()?
            .delegate(ToolInvocation::new(invocation_id, arguments), context)
            .await?;
        let run = HostRun {
            task_id,
            child_thread_id,
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
        self.contract()?
            .cancel(invocation_id, task_id, reason, context)
            .await
    }
}

/// A child receives just the rebinding command and the self-contained task.
/// No `target.model` is sent: T3 accepts only models the ACP agent advertises,
/// which excludes most pinned definition models (measured: `openai-codex/
/// gpt-6-luna` failed with `model_unavailable`). The child's `/agent:` rebind
/// applies the definition's model policy or the explicit per-call override itself.
fn delegate_arguments(
    definition: &AgentDefinition,
    prompt: &str,
    model_override: Option<&str>,
) -> Value {
    // Let T3 derive its display title from the task. Copying instructions into
    // the separate title field can exceed its budget and reject the launch.
    let task = match model_override {
        Some(model) => format!("/agent:{} --model {model} -- {prompt}", definition.id),
        None if prompt.trim_start().starts_with("--") => {
            format!("/agent:{} -- {prompt}", definition.id)
        }
        None => format!("/agent:{} {prompt}", definition.id),
    };
    json!({
        "task": task,
        "mode": "async",
    })
}

#[cfg(test)]
#[path = "host_orchestrator_tests.rs"]
mod tests;
