//! Delegated agent tools backed by in-process SDK runtimes.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use {
    crate::agent::{AgentCatalog, AgentDefinition},
    crate::app::agent_executor::{AgentExecutor, AgentLaunchRequest, AgentRunHandle},
    crate::subagent::{self, RunState, RunStatus},
    rho_tools::cancellation::RunCancellation,
    rho_tools::tool::{Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

use super::agent_output::{
    format_background_start, format_list_entry, format_running, format_snapshot, SnapshotFormat,
};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct SubagentSnapshot {
    pub id: String,
    pub agent_id: String,
    pub background: bool,
    pub elapsed: Duration,
    pub status: RunStatus,
    pub done: bool,
}

struct AgentEntry {
    agent_id: String,
    background: bool,
    started: Instant,
    handle: AgentRunHandle,
    session_id: Option<String>,
    notified: bool,
}

impl AgentEntry {
    fn snapshot(&self, id: &str) -> SubagentSnapshot {
        let status = self.handle.status();
        SubagentSnapshot {
            id: id.to_string(),
            agent_id: self.agent_id.clone(),
            background: self.background,
            elapsed: self.started.elapsed(),
            done: status.state.is_terminal(),
            status,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubagentNotification {
    pub snapshot: SubagentSnapshot,
}

#[derive(Clone)]
pub struct SubagentManager {
    inner: Arc<Mutex<HashMap<String, AgentEntry>>>,
    executor: AgentExecutor,
    session_id: Arc<Mutex<Option<String>>>,
}

impl SubagentManager {
    pub fn new(executor: AgentExecutor) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            executor,
            session_id: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_session(&self, session_id: String) {
        *self.session_id.lock().expect("delegated session lock") = Some(session_id);
    }

    pub fn update_model(&self, provider: &str, model: &str, reasoning: rho_sdk::ReasoningLevel) {
        self.executor.update_model(provider, model, reasoning);
    }

    /// Updates the policy snapshot used by future launches. Already-spawned
    /// agents retain the mode captured when they were launched.
    pub(crate) fn update_permission_mode(&self, mode: crate::permission::PermissionMode) {
        self.executor.update_permission_mode(mode);
    }

    #[cfg(test)]
    pub(crate) fn launch_permission_mode(&self) -> crate::permission::PermissionMode {
        self.executor.launch_permission_mode()
    }

    pub async fn spawn(
        &self,
        definition: &AgentDefinition,
        prompt: &str,
        background: bool,
        _cwd: &Path,
    ) -> anyhow::Result<(String, PathBuf)> {
        let (id, directory) = create_run_directory()?;
        let output_file = directory.join(subagent::RESULT_FILE_NAME);
        let session_id = self
            .session_id
            .lock()
            .expect("delegated session lock")
            .clone();
        let handle = self.executor.spawn(AgentLaunchRequest {
            definition: Arc::new(definition.clone()),
            prompt: prompt.to_string(),
            parent_session_id: session_id
                .as_deref()
                .and_then(|id| rho_sdk::SessionId::from_string(id.to_owned()).ok()),
            output_file,
        })?;
        self.inner.lock().expect("delegated registry lock").insert(
            id.clone(),
            AgentEntry {
                agent_id: definition.id.to_string(),
                background,
                started: Instant::now(),
                handle,
                session_id,
                notified: false,
            },
        );
        Ok((id, directory.join(subagent::LOG_FILE_NAME)))
    }

    pub fn status(&self, id: &str) -> Option<SubagentSnapshot> {
        self.inner
            .lock()
            .expect("delegated registry lock")
            .get(id)
            .map(|entry| entry.snapshot(id))
    }

    pub fn list(&self) -> Vec<SubagentSnapshot> {
        let entries = self.inner.lock().expect("delegated registry lock");
        let mut snapshots = entries
            .iter()
            .map(|(id, entry)| entry.snapshot(id))
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.elapsed));
        snapshots
    }

    pub fn has_active_or_pending_notification(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .expect("delegated registry lock")
            .iter()
            .any(|(id, entry)| {
                let snapshot = entry.snapshot(id);
                !snapshot.done
                    || (entry.background
                        && !entry.notified
                        && entry.session_id.as_deref() == Some(session_id))
            })
    }

    pub fn take_notifications(&self, session_id: &str) -> Vec<SubagentNotification> {
        self.inner
            .lock()
            .expect("delegated registry lock")
            .iter_mut()
            .filter_map(|(id, entry)| {
                let snapshot = entry.snapshot(id);
                (entry.background
                    && snapshot.done
                    && !entry.notified
                    && entry.session_id.as_deref() == Some(session_id))
                .then(|| {
                    entry.notified = true;
                    SubagentNotification { snapshot }
                })
            })
            .collect()
    }

    pub async fn wait_done(&self, id: &str) -> Option<SubagentSnapshot> {
        let mut handle = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .get(id)?
            .handle
            .clone();
        handle.wait().await;
        self.status(id)
    }

    pub async fn stop(&self, id: &str) -> anyhow::Result<SubagentSnapshot> {
        let mut handle = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown delegated run '{id}'"))?
            .handle
            .clone();
        handle.cancel();
        tokio::time::timeout(SHUTDOWN_TIMEOUT, handle.wait())
            .await
            .map_err(|_| anyhow::anyhow!("timed out stopping delegated run '{id}'"))?;
        self.status(id)
            .ok_or_else(|| anyhow::anyhow!("delegated run '{id}' disappeared"))
    }

    pub async fn shutdown(&self) {
        let handles = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .values()
            .map(|entry| entry.handle.clone())
            .collect::<Vec<_>>();
        for handle in &handles {
            handle.cancel();
        }
        let waits = handles.into_iter().map(|mut handle| async move {
            handle.wait().await;
        });
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, futures_util::future::join_all(waits)).await;
    }
}

fn new_agent_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    id[..6].to_string()
}

fn create_run_directory() -> anyhow::Result<(String, PathBuf)> {
    const MAX_ATTEMPTS: usize = 100;
    for _ in 0..MAX_ATTEMPTS {
        let id = new_agent_id();
        let directory = subagent::directory(&id)?;
        if let Some(parent) = directory.parent() {
            std::fs::create_dir_all(parent)?;
            subagent::secure_directory(parent)?;
        }
        match subagent::create_private_directory(&directory) {
            Ok(()) => return Ok((id, directory)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("could not allocate a unique delegated run ID")
}

pub fn notification_prompts(notification: &SubagentNotification) -> (String, String) {
    let snapshot = &notification.snapshot;
    let model = format!(
        "[agent notification]\n\n{}\n\nThis is an automated notification, not a user message. Read the `verification:` line above as the authoritative status of the delegated run; only a passing review counts as verified. Fold the result into your ongoing work; use the agents tool for details.",
        format_snapshot(snapshot, SnapshotFormat::Completion)
    );
    let display = format!(
        "agent {} ({}) finished - {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str()
    );
    (model, display)
}

pub(super) enum BackgroundSubagents {
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
}

impl AgentTool {
    pub(super) fn new(
        manager: SubagentManager,
        cwd: &Path,
        background_subagents: BackgroundSubagents,
    ) -> Self {
        let catalog =
            Arc::new(AgentCatalog::discover(cwd).expect("agent catalog was validated at startup"));
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
        }
    }
}

#[derive(Deserialize)]
struct AgentArgs {
    agent_id: String,
    prompt: String,
    #[serde(default)]
    background: bool,
}

#[async_trait]
impl Tool for AgentTool {
    fn spec(&self) -> ToolSpec {
        let names: Vec<&str> = self
            .agent_summaries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
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
                "description": "Run concurrently and return an id immediately"
            });
        }
        ToolSpec {
            name: "agent".into(),
            description: format!(
                "Delegate a substantial, self-contained task to a fresh agent. Background results start a new turn automatically. To wait for a background result, end the current turn. Do not call sleep or poll when no foreground work remains. Use `rho attach <id>` to watch the returned delegated run ID.\n\nAgents:\n{summaries}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": ["agent_id", "prompt"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let mut on_update = |_: Vec<String>| {};
        self.call_with_updates(args, ctx, id, &mut on_update).await
    }

    async fn call_with_updates(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        let args: AgentArgs = serde_json::from_value(args)?;
        if args.background && !self.background_subagents.is_enabled() {
            return Err(ToolError::Message(
                "background agents are unavailable in non-interactive runs".into(),
            ));
        }
        let definition = self
            .catalog
            .find(&args.agent_id)
            .map_err(|error| ToolError::Message(error.to_string()))?
            .definition
            .clone();
        let definition_id = definition.id.to_string();
        let (run_id, _log_file) = self
            .manager
            .spawn(&definition, &args.prompt, args.background, &ctx.cwd)
            .await
            .map_err(|error| {
                ToolError::Message(format!("failed to start delegated agent: {error}"))
            })?;

        if args.background {
            return Ok(ToolResult {
                id,
                ok: true,
                content: format_background_start(&run_id, &definition_id),
            });
        }

        on_update(vec![format_running(&run_id)]);
        let snapshot =
            self.manager.wait_done(&run_id).await.ok_or_else(|| {
                ToolError::Message(format!("delegated run '{run_id}' disappeared"))
            })?;
        Ok(ToolResult {
            id,
            ok: snapshot.status.state == RunState::Ok,
            content: format_snapshot(&snapshot, SnapshotFormat::Completion),
        })
    }

    async fn call_with_updates_and_cancellation(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
        cancellation: RunCancellation,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        // Foreground delegated runs must stop when the parent run is
        // interrupted instead of leaving an orphan behind.
        let background = args
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let call = self.call_with_updates(args, ctx, id, on_update);
        tokio::pin!(call);
        tokio::select! {
            result = &mut call => result,
            () = cancellation.cancelled() => {
                if background {
                    // Let an in-flight spawn finish registration so the
                    // manager retains ownership of the delegated task.
                    let _ = call.await;
                } else {
                    loop {
                        let running: Vec<String> = self
                            .manager
                            .list()
                            .into_iter()
                            .filter(|snapshot| !snapshot.done && !snapshot.background)
                            .map(|snapshot| snapshot.id)
                            .collect();
                        if !running.is_empty() {
                            for id in running {
                                let _ = self.manager.stop(&id).await;
                            }
                            break;
                        }
                        tokio::select! {
                            _ = &mut call => break,
                            () = tokio::time::sleep(POLL_INTERVAL) => {}
                        }
                    }
                }
                Err(ToolError::Message("tool interrupted".into()))
            }
        }
    }
}

pub struct AgentsTool {
    manager: SubagentManager,
}

impl AgentsTool {
    pub fn new(manager: SubagentManager) -> Self {
        Self { manager }
    }
}

#[derive(Deserialize)]
struct AgentsArgs {
    action: String,
    #[serde(default)]
    id: Option<String>,
}

#[async_trait]
impl Tool for AgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "agents".into(),
            description: "Check background-agent progress or stop a run. Completed results are delivered automatically. To wait for a result, end the current turn. Do not call sleep or poll when no foreground work remains.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "status", "stop"],
                        "description": "Operation to perform"
                    },
                    "id": {
                        "type": "string",
                        "description": "Delegated run ID (required for status and stop)"
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: AgentsArgs = serde_json::from_value(args)?;
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
                let snapshot = self
                    .manager
                    .status(id)
                    .ok_or_else(|| ToolError::Message(format!("unknown delegated run '{id}'")))?;
                format_snapshot(&snapshot, SnapshotFormat::Status)
            }
            "stop" => {
                let id = required_id(&args)?;
                let snapshot = self
                    .manager
                    .stop(id)
                    .await
                    .map_err(|error| ToolError::Message(error.to_string()))?;
                format_snapshot(&snapshot, SnapshotFormat::Status)
            }
            other => {
                return Err(ToolError::Message(format!(
                    "unknown action '{other}': expected list, status, or stop"
                )))
            }
        };
        Ok(ToolResult {
            id,
            ok: true,
            content,
        })
    }
}

fn required_id(args: &AgentsArgs) -> Result<&str, ToolError> {
    args.id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ToolError::Message("this action requires a delegated run id".into()))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
