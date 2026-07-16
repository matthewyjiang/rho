//! Subagent spawn (`agent`) and lifecycle (`agents`) tools.
//!
//! A subagent is a directly owned child `rho run --preset <name>` process.
//! Its output is teed to a log file and its structured status and display
//! events are persisted so any terminal can watch it with `rho attach <id>`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    cancellation::RunCancellation,
    subagent::{self, Preset, RunState, RunStatus},
    tool::{Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

use super::agent_output::{
    format_background_start, format_list_entry, format_running, format_snapshot, SnapshotFormat,
};

const POLL_INTERVAL: Duration = Duration::from_millis(300);
const STOP_GRACE: Duration = Duration::from_secs(5);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

type ForceKillRequest = tokio::sync::oneshot::Sender<Result<(), String>>;
type ForceKillSender = tokio::sync::mpsc::UnboundedSender<ForceKillRequest>;

enum StatusWatchOutcome {
    Continue,
    Finished,
    StartupTimedOut(ForceKillSender),
}

#[derive(Clone, Debug)]
pub struct SubagentSnapshot {
    pub id: String,
    pub preset: String,
    pub background: bool,
    pub elapsed: Duration,
    pub status: RunStatus,
    pub done: bool,
}

struct AgentEntry {
    preset: String,
    background: bool,
    started: Instant,
    output_file: PathBuf,
    force_kill: ForceKillSender,
    session_id: Option<String>,
    status: RunStatus,
    done: bool,
    process_exited: bool,
    notified: bool,
}

impl AgentEntry {
    fn snapshot(&self, id: &str) -> SubagentSnapshot {
        SubagentSnapshot {
            id: id.to_string(),
            preset: self.preset.clone(),
            background: self.background,
            elapsed: self.started.elapsed(),
            status: self.status.clone(),
            done: self.done,
        }
    }

    fn finish_with_error(&mut self, error: String) {
        self.status.state = RunState::Error;
        self.status.error = Some(error);
        let _ = subagent::write_status(&self.output_file, &self.status);
        self.done = true;
    }
}

/// Notification delivered to the host when a background subagent finishes.
#[derive(Clone, Debug)]
pub struct SubagentNotification {
    pub snapshot: SubagentSnapshot,
}

#[derive(Clone, Default)]
pub struct SubagentManager {
    inner: Arc<Mutex<HashMap<String, AgentEntry>>>,
    config_path: Option<PathBuf>,
    session_id: Arc<Mutex<Option<String>>>,
}

impl SubagentManager {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config_path(config_path: Option<PathBuf>) -> Self {
        Self {
            config_path,
            ..Self::default()
        }
    }

    pub fn set_session(&self, session_id: String) {
        *self.session_id.lock().expect("subagent session lock") = Some(session_id);
    }

    pub async fn spawn(
        &self,
        preset: &Preset,
        prompt: &str,
        background: bool,
        cwd: &Path,
    ) -> anyhow::Result<(String, PathBuf)> {
        let (id, dir) = create_run_directory()?;
        let output_file = dir.join(subagent::RESULT_FILE_NAME);
        let log_file = dir.join(subagent::LOG_FILE_NAME);

        let exe = std::env::current_exe()?;
        let args = run_args(
            &preset.name,
            &output_file,
            prompt,
            self.config_path.as_deref(),
        );
        let session_id = self
            .session_id
            .lock()
            .expect("subagent session lock")
            .clone();

        let child = spawn_headless(&exe, &args, cwd, &log_file)?;
        let force_kill = self.watch_child(&id, child);

        let entry = AgentEntry {
            preset: preset.name.clone(),
            background,
            started: Instant::now(),
            output_file: output_file.clone(),
            force_kill,
            session_id,
            status: RunStatus {
                preset: Some(preset.name.clone()),
                ..RunStatus::default()
            },
            done: false,
            process_exited: false,
            notified: false,
        };
        self.inner
            .lock()
            .expect("subagent registry lock")
            .insert(id.clone(), entry);
        self.watch_status_file(&id, output_file);
        Ok((id, log_file))
    }

    /// Polls the status file until it reaches a terminal state.
    fn watch_status_file(&self, id: &str, output_file: PathBuf) {
        let manager = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut seen_status = false;
            loop {
                tokio::time::sleep(POLL_INTERVAL).await;
                let status = subagent::read_status(&output_file);
                let outcome = {
                    let mut agents = manager.inner.lock().expect("subagent registry lock");
                    let Some(entry) = agents.get_mut(&id) else {
                        return;
                    };
                    if entry.done {
                        return;
                    }
                    if let Some(status) = status {
                        seen_status = true;
                        entry.status = status;
                    }
                    if !seen_status && started.elapsed() > STARTUP_TIMEOUT {
                        entry.finish_with_error(
                            "subagent never wrote its status file; it likely failed to start"
                                .into(),
                        );
                        StatusWatchOutcome::StartupTimedOut(entry.force_kill.clone())
                    } else if entry.status.state.is_terminal() {
                        entry.done = true;
                        StatusWatchOutcome::Finished
                    } else {
                        StatusWatchOutcome::Continue
                    }
                };
                match outcome {
                    StatusWatchOutcome::Continue => {}
                    StatusWatchOutcome::Finished => return,
                    StatusWatchOutcome::StartupTimedOut(force_kill) => {
                        let (ack, _killed) = tokio::sync::oneshot::channel();
                        let _ = force_kill.send(ack);
                        return;
                    }
                }
            }
        });
    }

    /// Marks the entry failed if the headless child exits without ever
    /// writing a terminal state.
    fn watch_child(&self, id: &str, mut child: tokio::process::Child) -> ForceKillSender {
        let (force_kill, mut force_kill_requests) =
            tokio::sync::mpsc::unbounded_channel::<ForceKillRequest>();
        let manager = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let (exit, force_kill_ack) = tokio::select! {
                exit = child.wait() => (exit, None),
                request = force_kill_requests.recv() => {
                    if let Some(ack) = request {
                        (kill_child_process_group(&mut child).await, Some(ack))
                    } else {
                        (child.wait().await, None)
                    }
                }
            };
            if let Some(ack) = force_kill_ack {
                let result = exit
                    .as_ref()
                    .map(|_| ())
                    .map_err(std::string::ToString::to_string);
                let _ = ack.send(result);
            }
            // Give the status-file watcher one poll interval to observe the
            // final write before synthesizing a failure.
            tokio::time::sleep(POLL_INTERVAL * 2).await;
            let mut agents = manager.inner.lock().expect("subagent registry lock");
            let Some(entry) = agents.get_mut(&id) else {
                return;
            };
            entry.process_exited = true;
            if entry.done || entry.status.state.is_terminal() {
                return;
            }
            if let Some(status) = subagent::read_status(&entry.output_file) {
                entry.status = status;
                if entry.status.state.is_terminal() {
                    entry.done = true;
                    return;
                }
            }
            let error = match exit {
                Ok(status) => format!("subagent exited ({status}) without writing a result"),
                Err(error) => format!("failed to wait for subagent: {error}"),
            };
            entry.finish_with_error(error);
        });
        force_kill
    }

    pub fn status(&self, id: &str) -> Option<SubagentSnapshot> {
        let agents = self.inner.lock().expect("subagent registry lock");
        agents.get(id).map(|entry| entry.snapshot(id))
    }

    pub fn list(&self) -> Vec<SubagentSnapshot> {
        let agents = self.inner.lock().expect("subagent registry lock");
        let mut list: Vec<_> = agents
            .iter()
            .map(|(id, entry)| entry.snapshot(id))
            .collect();
        list.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.elapsed));
        list
    }

    /// Checks active work and pending notifications under one lock so the TUI
    /// cannot miss a completion between two separate observations.
    pub fn has_active_or_pending_notification(&self, session_id: &str) -> bool {
        let agents = self.inner.lock().expect("subagent registry lock");
        agents.values().any(|entry| {
            !entry.done
                || (entry.background
                    && !entry.notified
                    && entry.session_id.as_deref() == Some(session_id))
        })
    }

    /// Returns finished background subagents for the current chat session that
    /// have not been announced yet, marking them announced.
    pub fn take_notifications(&self, session_id: &str) -> Vec<SubagentNotification> {
        let mut agents = self.inner.lock().expect("subagent registry lock");
        agents
            .iter_mut()
            .filter(|(_, entry)| {
                entry.background
                    && entry.done
                    && !entry.notified
                    && entry.session_id.as_deref() == Some(session_id)
            })
            .map(|(id, entry)| {
                entry.notified = true;
                SubagentNotification {
                    snapshot: entry.snapshot(id),
                }
            })
            .collect()
    }

    pub async fn wait_done(&self, id: &str) -> Option<SubagentSnapshot> {
        loop {
            let snapshot = self.status(id)?;
            if snapshot.done {
                return Some(snapshot);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Graceful stop: request cancellation through the run directory, wait
    /// up to [`STOP_GRACE`], then force-kill the child process.
    pub async fn stop(&self, id: &str) -> anyhow::Result<SubagentSnapshot> {
        let snapshot = self
            .status(id)
            .ok_or_else(|| anyhow::anyhow!("unknown subagent '{id}'"))?;
        let (output_file, process_exited) = {
            let agents = self.inner.lock().expect("subagent registry lock");
            let entry = agents
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("unknown subagent '{id}'"))?;
            (entry.output_file.clone(), entry.process_exited)
        };
        if process_exited {
            return Ok(snapshot);
        }
        if !snapshot.done {
            subagent::request_cancel(&output_file)?;
        }

        let grace = if snapshot.done {
            POLL_INTERVAL * 2
        } else {
            STOP_GRACE
        };
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            tokio::time::sleep(POLL_INTERVAL).await;
            let agents = self.inner.lock().expect("subagent registry lock");
            if let Some(entry) = agents.get(id).filter(|entry| entry.process_exited) {
                return Ok(entry.snapshot(id));
            }
        }

        let force_kill = {
            let agents = self.inner.lock().expect("subagent registry lock");
            agents
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("unknown subagent '{id}'"))?
                .force_kill
                .clone()
        };
        let (ack, killed) = tokio::sync::oneshot::channel();
        force_kill
            .send(ack)
            .map_err(|_| anyhow::anyhow!("subagent '{id}' process watcher stopped"))?;
        killed
            .await
            .map_err(|_| anyhow::anyhow!("subagent '{id}' kill was not acknowledged"))?
            .map_err(anyhow::Error::msg)?;

        let (snapshot, status) = {
            let mut agents = self.inner.lock().expect("subagent registry lock");
            let entry = agents
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("unknown subagent '{id}'"))?;
            if !entry.done {
                entry.status.state = RunState::Stopped;
                entry.status.error = Some("killed after stop grace period".into());
                entry.done = true;
            }
            entry.process_exited = true;
            (entry.snapshot(id), entry.status.clone())
        };
        let _ = subagent::write_status(&output_file, &status);
        Ok(snapshot)
    }

    /// Gracefully stops still-running children on shutdown.
    pub async fn shutdown(&self) {
        let ids: Vec<String> = self
            .inner
            .lock()
            .expect("subagent registry lock")
            .iter()
            .filter(|(_, entry)| !entry.process_exited)
            .map(|(id, _)| id.clone())
            .collect();
        let stops = ids.into_iter().map(|id| {
            let manager = self.clone();
            async move {
                let _ = manager.stop(&id).await;
            }
        });
        futures_util::future::join_all(stops).await;
    }
}

async fn kill_child_process_group(
    child: &mut tokio::process::Child,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let result = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        return child.wait().await;
    }

    child.kill().await?;
    child.wait().await
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
    anyhow::bail!("could not allocate a unique subagent id")
}

fn run_args(
    preset: &str,
    output_file: &Path,
    prompt: &str,
    config_path: Option<&Path>,
) -> Vec<String> {
    let mut args = vec!["--no-subagents".into()];
    if let Some(config_path) = config_path {
        args.extend([
            "--config".into(),
            config_path.to_string_lossy().into_owned(),
        ]);
    }
    args.extend([
        "run".into(),
        "--preset".into(),
        preset.into(),
        "--output-file".into(),
        output_file.to_string_lossy().into_owned(),
        "--".into(),
        prompt.into(),
    ]);
    args
}

fn spawn_headless(
    exe: &Path,
    args: &[String],
    cwd: &Path,
    log_file: &Path,
) -> anyhow::Result<tokio::process::Child> {
    let log = subagent::create_private_file(log_file)?;
    let stderr_log = log.try_clone()?;
    let mut command = tokio::process::Command::new(exe);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr_log)
        // The child is not occupying the parent's terminal, so it must not
        // report agent state against the parent's Herdr pane.
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_PANE_ID")
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    Ok(command.spawn()?)
}

pub fn notification_prompts(notification: &SubagentNotification) -> (String, String) {
    let snapshot = &notification.snapshot;
    let model = format!(
        "[subagent notification]\n\n{}\n\nThis is an automated notification, not a user message. Fold the result into your ongoing work; use the agents tool for details.",
        format_snapshot(snapshot, SnapshotFormat::Completion)
    );
    let display = format!(
        "subagent {} ({}) finished - {}",
        snapshot.id,
        snapshot.preset,
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
    preset_summaries: Vec<(String, String)>,
    background_subagents: BackgroundSubagents,
}

impl AgentTool {
    pub(super) fn new(
        manager: SubagentManager,
        cwd: &Path,
        background_subagents: BackgroundSubagents,
    ) -> Self {
        let preset_summaries = subagent::discover(cwd)
            .into_iter()
            .map(|preset| (preset.name, preset.description))
            .collect();
        Self {
            manager,
            preset_summaries,
            background_subagents,
        }
    }
}

#[derive(Deserialize)]
struct AgentArgs {
    preset: String,
    prompt: String,
    #[serde(default)]
    background: bool,
}

#[async_trait]
impl Tool for AgentTool {
    fn spec(&self) -> ToolSpec {
        let names: Vec<&str> = self
            .preset_summaries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let summaries = self
            .preset_summaries
            .iter()
            .map(|(name, description)| format!("{name}: {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut properties = json!({
            "preset": {
                "type": "string",
                "enum": names,
                "description": "Agent preset"
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
                "Delegate a substantial, self-contained task to a fresh agent. Background results start a new turn automatically. Do not poll or wait when no foreground work remains. Use `rho attach <id>` to watch the returned subagent ID.\n\nPresets:\n{summaries}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": ["preset", "prompt"],
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
                "background subagents are unavailable in non-interactive runs".into(),
            ));
        }
        let preset = subagent::find(&ctx.cwd, &args.preset)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        let (agent_id, _log_file) = self
            .manager
            .spawn(&preset, &args.prompt, args.background, &ctx.cwd)
            .await
            .map_err(|error| ToolError::Message(format!("failed to spawn subagent: {error}")))?;

        if args.background {
            return Ok(ToolResult {
                id,
                ok: true,
                content: format_background_start(&agent_id, &preset.name),
            });
        }

        on_update(vec![format_running(&agent_id)]);
        let snapshot = self
            .manager
            .wait_done(&agent_id)
            .await
            .ok_or_else(|| ToolError::Message(format!("subagent '{agent_id}' disappeared")))?;
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
        // Blocking spawns must stop their subagent when the run is
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
                    // manager retains ownership of the child process.
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
            description: "Check or stop background subagents.".into(),
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
                        "description": "Subagent id (required for status and stop)"
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
                    "no subagents".to_string()
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
                    .ok_or_else(|| ToolError::Message(format!("unknown subagent '{id}'")))?;
                format_snapshot(&snapshot, SnapshotFormat::Status)
            }
            "stop" => {
                let id = required_id(&args)?;
                let snapshot = self
                    .manager
                    .stop(id)
                    .await
                    .map_err(|error| ToolError::Message(error.to_string()))?;
                format_snapshot(&snapshot, SnapshotFormat::Completion)
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
        .ok_or_else(|| ToolError::Message("this action requires a subagent id".into()))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
