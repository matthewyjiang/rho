//! Delegated agent tools backed by in-process SDK runtimes.

use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use serde::Deserialize;
use serde_json::{json, Value};

use {
    crate::agent::AgentCatalog,
    crate::app::subagent_manager::ValidatedMessage,
    crate::subagent::RunState,
    rho_sdk::tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolError, ToolErrorKind, ToolInvocation,
        ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolProgress,
        ToolResource, ToolResourceAccess, ToolSecurity,
    },
};

use super::agent_output::{
    format_background_start, format_list_entry, format_running, format_snapshot, SnapshotFormat,
};

const SUBAGENT_MANAGER: &str = "subagents";
const AGENT_TOOL: &str = "agent";
const AGENTS_TOOL: &str = "agents";

pub use crate::app::subagent_manager::{SubagentManager, SubagentNotification, SubagentSnapshot};

pub(crate) use super::agent_output::merge_notification_context;
pub use super::agent_output::notification_prompts;
#[cfg(test)]
pub(crate) use super::agent_output::MODEL_NOTIFICATION_BYTES as NOTIFICATION_CONTEXT_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackgroundSubagents {
    Disabled,
    Enabled,
}

impl BackgroundSubagents {
    fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

pub struct AgentTool {
    manager: SubagentManager,
    catalog: Arc<AgentCatalog>,
    agent_summaries: Vec<(String, String)>,
    background_subagents: BackgroundSubagents,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
}

impl AgentTool {
    pub(super) fn new(
        manager: SubagentManager,
        cwd: &Path,
        background_subagents: BackgroundSubagents,
        catalog: Option<Arc<AgentCatalog>>,
    ) -> Self {
        let catalog = catalog.unwrap_or_else(|| {
            Arc::new(AgentCatalog::discover(cwd).expect("agent catalog was validated at startup"))
        });
        let agent_summaries = catalog
            .iter()
            .filter(|entry| entry.definition.id.as_str() != "default")
            .map(|entry| {
                (
                    entry.definition.id.to_string(),
                    entry.definition.description.clone(),
                )
            })
            .collect();
        Self {
            manager,
            catalog,
            agent_summaries,
            background_subagents,
            mutation_observer: Arc::new(()),
        }
    }

    fn with_mutation_observer(
        mut self,
        mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
    ) -> Self {
        self.mutation_observer = mutation_observer;
        self
    }

    async fn execute(
        &self,
        args: AgentArgs,
        context: &rho_sdk::tool::AuthorizedToolContext,
    ) -> Result<ToolOutput, ToolError> {
        if args.background && !self.background_subagents.is_enabled() {
            return Err(ToolError::new(
                ToolErrorKind::InvalidArguments,
                "background agents are unavailable in non-interactive runs",
            ));
        }

        let definition = self
            .catalog
            .find(&args.agent_id)
            .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?
            .definition
            .clone();
        let definition_id = definition.id.to_string();
        self.mutation_observer
            .mark_untracked_effect(rho_tools::UntrackedWorkspaceEffect::MutatingTool, "agent");
        let cwd = context
            .workspace_root()
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        let spawn = self
            .manager
            .spawn(&definition, &args.prompt, args.background, &cwd);
        tokio::pin!(spawn);
        let (run_id, _log_file) = tokio::select! {
            result = &mut spawn => result.map_err(|error| {
                ToolError::new(
                    ToolErrorKind::Execution,
                    format!("failed to start delegated agent: {error}"),
                )
            })?,
            () = context.cancellation().cancelled() => {
                if args.background {
                    // Let an in-flight spawn finish registration so the manager
                    // retains ownership of the delegated task.
                    let _ = spawn.await;
                }
                return Err(ToolError::cancelled());
            }
        };

        if args.background {
            // Registration is the start receipt; instant failures still reach
            // the parent through automatic completion delivery.
            return Ok(
                ToolOutput::text(format_background_start(&run_id, &definition_id))
                    .metadata(agent_metadata()),
            );
        }

        let _ = context
            .progress()
            .send(ToolProgress::message(format_running(&run_id)))
            .await;

        let wait = self.manager.wait_done(&run_id);
        tokio::pin!(wait);
        let snapshot = tokio::select! {
            snapshot = &mut wait => snapshot.ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::Execution,
                    format!("delegated run '{run_id}' disappeared"),
                )
            })?,
            () = context.cancellation().cancelled() => {
                // This invocation owns run_id for the wait. Stop only that
                // handle on parent cancellation.
                let _ = self.manager.stop(&run_id).await;
                return Err(ToolError::cancelled());
            }
        };

        let content = format_snapshot(&snapshot, SnapshotFormat::Completion);
        if snapshot.status.state != RunState::Ok {
            return Err(ToolError::new(ToolErrorKind::Execution, content));
        }
        Ok(ToolOutput::text(content).metadata(agent_metadata()))
    }
}

#[derive(Deserialize)]
struct AgentArgs {
    agent_id: String,
    prompt: String,
    #[serde(default)]
    background: bool,
}

impl Tool for AgentTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        let names: Vec<&str> = self
            .agent_summaries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        // Deliberately model-free. Which model an agent runs on can change after
        // this list is written - the conversation model switches, a catalog name
        // arrives - and rewriting the list would change what the caller was
        // already told. Each run reports its own model when it starts instead.
        let summaries = self
            .agent_summaries
            .iter()
            .map(|(name, description)| format!("{name}: {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut properties = json!({
            "agent_id": {
                "type": "string",
                "enum": names,
                "description": "Agent ID"
            },
            "prompt": {
                "type": "string",
                "description": "Self-contained task and all context the agent needs"
            }
        });
        if self.background_subagents.is_enabled() {
            properties["background"] = json!({
                "type": "boolean",
                "description": "Starts the run and returns an id immediately instead of waiting. Omit or set false to wait for the final result. Only background=true backgrounds a run; parallel batching does not. Independent agent calls in the same batch run together either way."
            });
        }
        // Parallel batch behavior is always true; background delivery text is
        // capability-gated so disabled runs do not advertise a missing path.
        let parallel_guidance =
            " Independent agent calls in the same batch run together - issue them in one turn for parallel work.";
        let background_guidance = if self.background_subagents.is_enabled() {
            " Foreground calls (background omitted or false) wait for completion. Issuing a foreground agent beside other tools does not background it and can delay the rest of that batch until the run finishes. Set background=true to start a run and return an id immediately; completions arrive automatically at the next turn boundary (multiple completions are batched in one notification). After starting background runs, end your turn once no other work remains - never sleep or poll for results."
        } else {
            " Calls wait for completion. Issuing an agent beside other tools can delay the rest of that batch until the run finishes."
        };
        rho_sdk::model::ToolSpec {
            name: AGENT_TOOL.into(),
            description: format!(
                "Delegate a substantial, self-contained task to a fresh agent.{parallel_guidance}{background_guidance}\n\nAgents:\n{summaries}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": ["agent_id", "prompt"],
                "additionalProperties": false
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let args = parse_agent_args(invocation.into_arguments());
        Box::pin(async move {
            let args = args?;
            // Registry ops are mutex-protected and short. Shared access lets
            // several launches in one batch overlap; each call still waits
            // only on its own handle when foreground.
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::shared(ToolResource::manager_state(
                    SUBAGENT_MANAGER,
                ))],
                [],
                agent_metadata(),
                move |context| Box::pin(async move { self.execute(args, &context).await }),
            ))
        })
    }
}

pub struct AgentsTool {
    manager: SubagentManager,
}

impl AgentsTool {
    pub fn new(manager: SubagentManager) -> Self {
        Self { manager }
    }

    async fn execute(&self, args: AgentsArgs) -> Result<ToolOutput, ToolError> {
        let content = match args.action.as_str() {
            "list" => {
                let agents = self.manager.list();
                if agents.is_empty() {
                    "no delegated agents".to_string()
                } else {
                    agents
                        .iter()
                        .map(format_list_entry)
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            "status" => {
                let id = required_id(&args)?;
                let snapshot = self.manager.observe(id).ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        format!("unknown delegated run '{id}'"),
                    )
                })?;
                // A finished run hands over its full result here and counts
                // as delivered; a running run reports progress only.
                let format = if snapshot.done {
                    SnapshotFormat::Completion
                } else {
                    SnapshotFormat::Status
                };
                format_snapshot(&snapshot, format)
            }
            "stop" => {
                let id = required_id(&args)?;
                let snapshot =
                    self.manager.stop(id).await.map_err(|error| {
                        ToolError::new(ToolErrorKind::Execution, error.to_string())
                    })?;
                format_snapshot(&snapshot, SnapshotFormat::Completion)
            }
            "message" => {
                let id = required_id(&args)?;
                let text = args.message.as_deref().ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "message action requires message text",
                    )
                })?;
                // Parse at this argument boundary so an empty or over-budget
                // body reads as invalid arguments, not an execution failure.
                let message = ValidatedMessage::parse(text).map_err(|error| {
                    ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
                })?;
                self.manager
                    .message(id, &message)
                    .await
                    .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
                format!("queued parent message for delegated run '{id}'")
            }
            other => {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    format!("unknown action '{other}': expected list, status, stop, or message"),
                ))
            }
        };
        Ok(ToolOutput::text(content).metadata(agents_metadata()))
    }
}

#[derive(Deserialize)]
struct AgentsArgs {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl Tool for AgentsTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: AGENTS_TOOL.into(),
            description: "Check on, stop, or message a delegated background run. Completions and child notices are delivered automatically at the next turn boundary (batched into one notification when several finish), so waiting for a result means ending your turn, not calling status. While a run is in progress, status reports progress only and never partial output - do not act on a run's result before it finishes. Once a run has finished, status or stop returns its final result and counts as delivery, so it will not be redelivered automatically. Use action=message to steer a running child with plain text: Rho-runtime children apply it at the next provider turn; claude-cli children receive it as a queued stream-json user turn; cursor children cannot be messaged (process-per-turn).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "status", "stop", "message"],
                        "description": "Operation to perform"
                    },
                    "id": {
                        "type": "string",
                        "description": "Delegated run ID (required for status, stop, and message)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Plain-text parent message (required for message). Rho children apply it at the next provider turn; Claude-cli children queue it as the next stdin user turn; cursor children cannot be messaged (process-per-turn)."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let args = parse_agents_args(invocation.into_arguments());
        Box::pin(async move {
            let args = args?;
            // list/status/stop all touch the mutex-backed registry briefly.
            // Shared access keeps lifecycle ops from serializing launches.
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::shared(ToolResource::manager_state(
                    SUBAGENT_MANAGER,
                ))],
                [],
                agents_metadata(),
                move |_context| Box::pin(async move { self.execute(args).await }),
            ))
        })
    }
}

fn parse_agent_args(arguments: Value) -> Result<AgentArgs, ToolError> {
    serde_json::from_value(arguments)
        .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))
}

fn parse_agents_args(arguments: Value) -> Result<AgentsArgs, ToolError> {
    serde_json::from_value(arguments)
        .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))
}

fn agent_metadata() -> ToolMetadata {
    ToolMetadata::new().operation(OperationKind::Other(AGENT_TOOL.into()))
}

fn agents_metadata() -> ToolMetadata {
    ToolMetadata::new().operation(OperationKind::Other(AGENTS_TOOL.into()))
}

fn required_id(args: &AgentsArgs) -> Result<&str, ToolError> {
    args.id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "this action requires a delegated run id",
            )
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DelegationToolSelection {
    Launch,
    Manage,
    LaunchAndManage,
}

impl DelegationToolSelection {
    pub(super) fn from_capabilities(
        capabilities: &crate::agent::AgentCapabilities,
    ) -> Option<Self> {
        use crate::agent::ToolCapability;

        match (
            capabilities.contains(&ToolCapability::Agent),
            capabilities.contains(&ToolCapability::Agents),
        ) {
            (true, true) => Some(Self::LaunchAndManage),
            (true, false) => Some(Self::Launch),
            (false, true) => Some(Self::Manage),
            (false, false) => None,
        }
    }

    fn launches(self) -> bool {
        matches!(self, Self::Launch | Self::LaunchAndManage)
    }

    fn manages(self) -> bool {
        matches!(self, Self::Manage | Self::LaunchAndManage)
    }
}

pub(super) struct DelegationBundleOptions {
    pub cwd: PathBuf,
    pub tools: DelegationToolSelection,
    pub config_path: PathBuf,
    pub background: BackgroundSubagents,
    /// Catalog already discovered for `cwd`; rediscovered when absent.
    pub catalog: Option<Arc<AgentCatalog>>,
}

pub(super) struct SdkDelegationBundle {
    tools: Vec<Arc<dyn rho_sdk::tool::Tool>>,
    manager: SubagentManager,
}

impl SdkDelegationBundle {
    pub(super) fn manager_handle(&self) -> SubagentManager {
        self.manager.clone()
    }
}

impl super::sdk_registry::ToolBundle for SdkDelegationBundle {
    fn tools(&self) -> &[Arc<dyn rho_sdk::tool::Tool>] {
        &self.tools
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.manager.shutdown())
    }
}

pub(super) fn sdk_bundle(
    config: &crate::config::Config,
    options: DelegationBundleOptions,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
) -> SdkDelegationBundle {
    let manager = SubagentManager::new(config.clone(), options.config_path, options.cwd.clone());
    let mut tools = Vec::<Arc<dyn rho_sdk::tool::Tool>>::new();
    if options.tools.launches() {
        tools.push(Arc::new(
            AgentTool::new(
                manager.clone(),
                &options.cwd,
                options.background,
                options.catalog,
            )
            .with_mutation_observer(mutation_observer),
        ));
    }
    if options.tools.manages() {
        tools.push(Arc::new(AgentsTool::new(manager.clone())));
    }
    SdkDelegationBundle { tools, manager }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
