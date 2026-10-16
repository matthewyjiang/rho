//! The ACP host's launch/manage tools. Transport and task lifetime stay in T3.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_sdk::{
    model::ToolSpec,
    tool::{
        OperationKind, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
        ToolMetadata, ToolOutput, ToolSecurity,
    },
    ToolCallId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    agent_metadata, agent_spec, agents_metadata,
    host_orchestrator::{HostOrchestrator, HostRun},
    parse_agent_args,
};
use crate::{
    agent::{AdvertisedAgents, AdvertisedChange, AgentCatalog, AgentDefinition, AgentRuntimeSpec},
    tools::{agent_output::AgentRunView, sdk_registry::AppToolSet},
};

/// `rho acp --t3code` hook, called after MCP attachment: swaps the native
/// `agent`/`agents` tools for T3-backed ones. A session without the native
/// `agent` tool (a delegated child, `--no-subagents`) is left unchanged. Missing
/// T3 tools or an unreadable agent catalog fail the session build.
pub(crate) fn install_host_delegation(
    tools: &mut AppToolSet,
    cwd: &Path,
    max_output_bytes: usize,
) -> anyhow::Result<()> {
    if !tools.contains("agent") {
        return Ok(());
    }
    let orchestrator = Arc::new(HostOrchestrator::connect(tools)?);
    let catalog = AgentCatalog::discover(cwd)?;
    let agent = HostAgentTool::new(
        orchestrator.clone(),
        cwd,
        &catalog,
        max_output_bytes,
        tools.checkpoint_tracker().clone(),
    );
    tools.replace_delegation_tools([
        Arc::new(agent),
        Arc::new(HostAgentsTool {
            orchestrator,
            max_output_bytes,
        }),
    ]);
    Ok(())
}

struct HostAgentTool {
    orchestrator: Arc<HostOrchestrator>,
    cwd: PathBuf,
    advertised: AdvertisedAgents,
    max_output_bytes: usize,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
}

impl HostAgentTool {
    fn new(
        orchestrator: Arc<HostOrchestrator>,
        cwd: &Path,
        catalog: &AgentCatalog,
        max_output_bytes: usize,
        mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
    ) -> Self {
        let mut advertised = AdvertisedAgents::from_catalog(catalog);
        for entry in catalog.iter() {
            match &entry.definition.runtime {
                AgentRuntimeSpec::Rho { .. } => {}
                AgentRuntimeSpec::ClaudeCli(_)
                | AgentRuntimeSpec::Cursor(_)
                | AgentRuntimeSpec::Antigravity(_) => {
                    advertised.apply(&AdvertisedChange::Unavailable {
                        agent_id: entry.definition.id.to_string(),
                    });
                }
            }
        }
        Self {
            orchestrator,
            cwd: cwd.to_owned(),
            advertised,
            max_output_bytes,
            mutation_observer,
        }
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let args = parse_agent_args(invocation.arguments().clone())?;
        let catalog = AgentCatalog::discover(&self.cwd).map_err(|error| {
            ToolError::new(
                ToolErrorKind::Execution,
                format!("could not reload agent definitions: {error}"),
            )
        })?;
        let definition = &catalog
            .find(&args.agent_id)
            .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?
            .definition;
        self.launch(invocation.id().clone(), definition, &args.prompt, context)
            .await
    }

    /// Enforces the host-runtime gate for the freshly resolved definition,
    /// then records the host's receipt without registering native child work.
    async fn launch(
        &self,
        invocation_id: ToolCallId,
        definition: &AgentDefinition,
        prompt: &str,
        context: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        // Recheck at launch: the on-disk runtime may have changed since spec().
        match &definition.runtime {
            AgentRuntimeSpec::Rho { .. } => {}
            AgentRuntimeSpec::ClaudeCli(_)
            | AgentRuntimeSpec::Cursor(_)
            | AgentRuntimeSpec::Antigravity(_) => {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    format!(
                        "{} runs on {}, which a host-managed Rho ACP child cannot host; \
                         call T3's delegate_task tool directly to start another provider",
                        definition.id,
                        definition.runtime.runtime()
                    ),
                ));
            }
        }
        self.mutation_observer
            .mark_untracked_effect(rho_tools::UntrackedWorkspaceEffect::MutatingTool, "agent");
        let run = self
            .orchestrator
            .delegate(invocation_id, definition, prompt, context)
            .await?;
        rho_tools::Rendered::new(
            format!(
                "agent {} ({}) started in background through the host",
                run.task_id, run.agent_id
            ),
            AgentRunView::started(run.task_id, run.agent_id),
        )
        .limit_data(self.max_output_bytes)?
        .into_tool_output(agent_metadata())
    }
}

impl Tool for HostAgentTool {
    fn spec(&self) -> ToolSpec {
        agent_spec(
            &self.advertised,
            " The host runs the child in the background and returns its task id immediately. The result arrives as your next turn; continue independent work or end your turn to wait. Never sleep or poll for results, and do not declare the task complete before required children finish. Parent cancellation and shutdown do not stop host-managed children; use agents stop explicitly.",
        )
    }

    fn output_schema(&self) -> Option<Value> {
        Some(rho_tools::output_schema::<AgentRunView>())
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn start_metadata(&self, _arguments: &Value) -> ToolMetadata {
        agent_metadata()
    }

    // MCP calls require the legacy context to drive the remote prepare/auth path.
    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(self.execute(invocation, context))
    }
}

struct HostAgentsTool {
    orchestrator: Arc<HostOrchestrator>,
    max_output_bytes: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
struct HostRunView {
    task_id: String,
    child_thread_id: Option<String>,
    agent_id: String,
    title: String,
}

impl From<&HostRun> for HostRunView {
    fn from(run: &HostRun) -> Self {
        Self {
            task_id: run.task_id.clone(),
            child_thread_id: run.child_thread_id.clone(),
            agent_id: run.agent_id.clone(),
            title: run.title.clone(),
        }
    }
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
enum HostAgentsOutput {
    List { runs: Vec<HostRunView> },
    Status { task_id: String, host: Value },
    Stop { task_id: String, host: Value },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum HostAction {
    List,
    Status,
    Stop,
    // Accepted at the argument boundary to return explicit backend guidance.
    Message,
}

#[derive(Deserialize)]
struct HostAgentsArgs {
    action: HostAction,
    id: Option<String>,
    reason: Option<String>,
}

impl HostAgentsTool {
    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let args: HostAgentsArgs = serde_json::from_value(invocation.arguments().clone())
            .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?;
        let (task_id, output, data) = match args.action {
            HostAction::List => {
                let runs = self.orchestrator.runs();
                let content = if runs.is_empty() {
                    "no host-managed agents".to_owned()
                } else {
                    runs.iter()
                        .map(|run| {
                            format!(
                                "{} ({}) — {} (started {}s ago; host owns status)",
                                run.task_id,
                                run.agent_id,
                                run.title,
                                run.started.elapsed().as_secs()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                return rho_tools::Rendered::new(
                    content,
                    HostAgentsOutput::List {
                        runs: runs.iter().map(HostRunView::from).collect(),
                    },
                )
                .limit_data(self.max_output_bytes)?
                .into_tool_output(agents_metadata());
            }
            HostAction::Message => {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "message is not available for host-managed children; stop and delegate again",
                ));
            }
            HostAction::Status => {
                let task_id = required_id(args.id)?;
                let output = self
                    .orchestrator
                    .status(invocation.id().clone(), &task_id, context)
                    .await?;
                let data = HostAgentsOutput::Status {
                    task_id: task_id.clone(),
                    host: output.structured_content().cloned().unwrap_or(Value::Null),
                };
                (task_id, output, data)
            }
            HostAction::Stop => {
                let task_id = required_id(args.id)?;
                let output = self
                    .orchestrator
                    .cancel(
                        invocation.id().clone(),
                        &task_id,
                        args.reason.as_deref(),
                        context,
                    )
                    .await?;
                let data = HostAgentsOutput::Stop {
                    task_id: task_id.clone(),
                    host: output.structured_content().cloned().unwrap_or(Value::Null),
                };
                (task_id, output, data)
            }
        };
        // Preserve host text, opaque structured status, assets/notices and
        // completed-failure semantics rather than inventing native RunStates.
        // T3 authorizes task ownership. A reloaded session has no local roster
        // but can still manage its durable host task IDs.
        let label = self
            .orchestrator
            .run(&task_id)
            .map_or(task_id, |run| run.agent_id);
        let rendered = rho_tools::Rendered::new(format!("{label}: {}", output.content()), data)
            .limit_data(self.max_output_bytes)?
            .into_tool_output(
                output
                    .presentation()
                    .clone()
                    .operation(OperationKind::Other("agents".into())),
            )?
            .with_images(output.images().to_vec());
        Ok(if output.is_failure() {
            rendered.failed()
        } else {
            rendered
        })
    }
}

fn required_id(id: Option<String>) -> Result<String, ToolError> {
    id.filter(|id| !id.is_empty()).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidArguments,
            "this action requires a host task id",
        )
    })
}

impl Tool for HostAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "agents".into(),
            description: "List locally launched host-managed children, inspect host status, or stop a child explicitly. Results arrive automatically as the host's next turn: end your turn to wait, never poll. Parent cancellation and session shutdown leave these children running. Messages are unavailable; stop and delegate again to change a task. The host owns durable history; this list resets when the session reloads.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "status", "stop"] },
                    "id": { "type": "string", "description": "Host task ID (required for status and stop)" },
                    "reason": { "type": "string", "description": "Optional reason for stopping the child" }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        }
    }

    fn output_schema(&self) -> Option<Value> {
        Some(rho_tools::output_schema::<HostAgentsOutput>())
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn start_metadata(&self, _arguments: &Value) -> ToolMetadata {
        agents_metadata()
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(self.execute(invocation, context))
    }
}

#[cfg(test)]
#[path = "host_tools_tests.rs"]
mod tests;
