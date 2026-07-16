//! Subagent spawn (`agent`) and lifecycle (`agents`) tools.
//!
//! A subagent is a child `rho run --preset <name>` process. Inside herdr the
//! pane is spawned by herdr itself (via the `herdr` CLI) so the user can watch
//! and scroll it; elsewhere the child runs headless with output teed to a log
//! file. Results always flow back through the structured status file
//! (`--output-file`), never by reading pane or log output.

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
    subagent::{self, OnExit, Preset, RunState, RunStatus},
    tool::{truncate, Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

const POLL_INTERVAL: Duration = Duration::from_millis(300);
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long a spawned subagent may go without writing its status file before
/// it is presumed dead (relevant to pane spawns, which have no child handle).
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// Cap on the result text echoed into notifications and blocking returns.
const RESULT_EXCERPT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnDisplay {
    /// Herdr owns the subagent's pane.
    Pane(String),
    /// Headless child; output tees to this log file.
    Log(PathBuf),
}

#[derive(Clone, Debug)]
pub struct SubagentSnapshot {
    pub id: String,
    pub preset: String,
    pub background: bool,
    pub elapsed: Duration,
    pub display: SpawnDisplay,
    pub status: RunStatus,
    pub done: bool,
}

struct AgentEntry {
    preset: String,
    background: bool,
    started: Instant,
    display: SpawnDisplay,
    on_exit: OnExit,
    pid: Option<u32>,
    status: RunStatus,
    done: bool,
    notified: bool,
}

impl AgentEntry {
    fn snapshot(&self, id: &str) -> SubagentSnapshot {
        SubagentSnapshot {
            id: id.to_string(),
            preset: self.preset.clone(),
            background: self.background,
            elapsed: self.started.elapsed(),
            display: self.display.clone(),
            status: self.status.clone(),
            done: self.done,
        }
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
}

impl SubagentManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn spawn(
        &self,
        preset: &Preset,
        prompt: &str,
        background: bool,
        cwd: &Path,
    ) -> anyhow::Result<(String, SpawnDisplay)> {
        let id = new_agent_id();
        let dir = subagent_dir(&id)?;
        std::fs::create_dir_all(&dir)?;
        let output_file = dir.join(subagent::RESULT_FILE_NAME);
        let log_file = dir.join(subagent::LOG_FILE_NAME);

        let exe = std::env::current_exe()?;
        let args = run_args(&preset.name, &output_file, prompt);

        let (display, pid) = match spawn_in_herdr_pane(&exe, &args, cwd).await {
            Ok(pane_id) => (SpawnDisplay::Pane(pane_id), None),
            Err(_) => {
                let (child_pid, child) = spawn_headless(&exe, &args, cwd, &log_file)?;
                self.watch_child(&id, child);
                (SpawnDisplay::Log(log_file), child_pid)
            }
        };

        let entry = AgentEntry {
            preset: preset.name.clone(),
            background,
            started: Instant::now(),
            display: display.clone(),
            on_exit: preset.on_exit,
            pid,
            status: RunStatus {
                preset: Some(preset.name.clone()),
                ..RunStatus::default()
            },
            done: false,
            notified: false,
        };
        self.inner
            .lock()
            .expect("subagent registry lock")
            .insert(id.clone(), entry);
        self.watch_status_file(&id, output_file);
        Ok((id, display))
    }

    /// Polls the status file until it reaches a terminal state. A subagent
    /// that never writes its status file (e.g. its pane command failed before
    /// rho started) is marked failed after [`STARTUP_TIMEOUT`] so blocking
    /// callers cannot wait forever.
    fn watch_status_file(&self, id: &str, output_file: PathBuf) {
        let manager = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut seen_status = false;
            loop {
                tokio::time::sleep(POLL_INTERVAL).await;
                let status = subagent::read_status(&output_file);
                let finished = {
                    let mut agents = manager.inner.lock().expect("subagent registry lock");
                    let Some(entry) = agents.get_mut(&id) else {
                        return;
                    };
                    if entry.done {
                        return;
                    }
                    if let Some(status) = status {
                        seen_status = true;
                        if entry.pid.is_none() {
                            entry.pid = status.pid;
                        }
                        entry.status = status;
                    } else if !seen_status && started.elapsed() > STARTUP_TIMEOUT {
                        entry.status.state = RunState::Error;
                        entry.status.error = Some(
                            "subagent never wrote its status file; it likely failed to start"
                                .into(),
                        );
                    }
                    if entry.status.state.is_terminal() {
                        entry.done = true;
                    }
                    entry.done.then(|| entry.snapshot(&id))
                };
                if let Some(snapshot) = finished {
                    manager.handle_exit_display(&snapshot).await;
                    return;
                }
            }
        });
    }

    /// Marks the entry failed if the headless child exits without ever
    /// writing a terminal state.
    fn watch_child(&self, id: &str, mut child: tokio::process::Child) {
        let manager = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let exit = child.wait().await;
            // Give the status-file watcher one poll interval to observe the
            // final write before synthesizing a failure.
            tokio::time::sleep(POLL_INTERVAL * 2).await;
            let mut agents = manager.inner.lock().expect("subagent registry lock");
            let Some(entry) = agents.get_mut(&id) else {
                return;
            };
            if entry.done || entry.status.state.is_terminal() {
                return;
            }
            entry.status.state = RunState::Error;
            entry.status.error = Some(match exit {
                Ok(status) => format!("subagent exited ({status}) without writing a result"),
                Err(error) => format!("failed to wait for subagent: {error}"),
            });
            entry.done = true;
        });
    }

    async fn handle_exit_display(&self, snapshot: &SubagentSnapshot) {
        let SpawnDisplay::Pane(pane_id) = &snapshot.display else {
            return;
        };
        let entry_on_exit = {
            let agents = self.inner.lock().expect("subagent registry lock");
            agents.get(&snapshot.id).map(|entry| entry.on_exit)
        };
        let close = match entry_on_exit {
            Some(OnExit::Close) => true,
            Some(OnExit::CloseOnSuccess) => snapshot.status.state == RunState::Ok,
            Some(OnExit::Keep) | None => false,
        };
        if close {
            let _ = tokio::process::Command::new("herdr")
                .args(["pane", "close", pane_id])
                .output()
                .await;
        }
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

    pub fn has_active(&self) -> bool {
        let agents = self.inner.lock().expect("subagent registry lock");
        agents.values().any(|entry| !entry.done)
    }

    /// Returns finished background subagents that have not been announced to
    /// the host yet, marking them announced.
    pub fn take_notifications(&self) -> Vec<SubagentNotification> {
        let mut agents = self.inner.lock().expect("subagent registry lock");
        agents
            .iter_mut()
            .filter(|(_, entry)| entry.background && entry.done && !entry.notified)
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

    /// Graceful stop: cancel signal, wait up to [`STOP_GRACE`], then kill.
    pub async fn stop(&self, id: &str) -> anyhow::Result<SubagentSnapshot> {
        let snapshot = self
            .status(id)
            .ok_or_else(|| anyhow::anyhow!("unknown subagent '{id}'"))?;
        if snapshot.done {
            return Ok(snapshot);
        }
        let pid = snapshot.status.pid.or_else(|| {
            let agents = self.inner.lock().expect("subagent registry lock");
            agents.get(id).and_then(|entry| entry.pid)
        });
        let Some(pid) = pid else {
            anyhow::bail!(
                "subagent '{id}' has not reported a pid yet; try again in a moment or close its pane manually"
            );
        };

        send_signal(pid, Signal::Interrupt)?;
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Some(snapshot) = self.status(id) {
                if snapshot.done {
                    return Ok(snapshot);
                }
            }
        }
        let _ = send_signal(pid, Signal::Kill);
        let mut agents = self.inner.lock().expect("subagent registry lock");
        let entry = agents
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("unknown subagent '{id}'"))?;
        if !entry.done {
            entry.status.state = RunState::Stopped;
            entry.status.error = Some("killed after stop grace period".into());
            entry.done = true;
        }
        Ok(entry.snapshot(id))
    }

    /// Kills still-running headless children on shutdown. Pane subagents are
    /// user-visible in herdr and are left running.
    pub fn shutdown(&self) {
        let agents = self.inner.lock().expect("subagent registry lock");
        for entry in agents.values() {
            if entry.done || matches!(entry.display, SpawnDisplay::Pane(_)) {
                continue;
            }
            if let Some(pid) = entry.pid.or(entry.status.pid) {
                let _ = send_signal(pid, Signal::Interrupt);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Signal {
    Interrupt,
    Kill,
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: Signal) -> anyhow::Result<()> {
    let signal = match signal {
        Signal::Interrupt => libc::SIGINT,
        Signal::Kill => libc::SIGKILL,
    };
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if result != 0 {
        anyhow::bail!("failed to signal subagent pid {pid}");
    }
    Ok(())
}

#[cfg(not(unix))]
fn send_signal(_pid: u32, _signal: Signal) -> anyhow::Result<()> {
    anyhow::bail!("stopping subagents is not supported on this platform yet")
}

fn new_agent_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    id[..6].to_string()
}

fn subagent_dir(id: &str) -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("subagents").join(id))
}

fn run_args(preset: &str, output_file: &Path, prompt: &str) -> Vec<String> {
    vec![
        "--no-subagents".into(),
        "run".into(),
        "--preset".into(),
        preset.into(),
        "--output-file".into(),
        output_file.to_string_lossy().into_owned(),
        "--".into(),
        prompt.into(),
    ]
}

fn herdr_pane_env() -> Option<String> {
    if !cfg!(unix) || std::env::var("HERDR_ENV").ok()?.as_str() != "1" {
        return None;
    }
    std::env::var("HERDR_PANE_ID").ok()
}

/// Asks herdr (over its CLI, which speaks the local socket) to split a pane
/// next to ours and run the subagent command in it.
async fn spawn_in_herdr_pane(exe: &Path, args: &[String], cwd: &Path) -> anyhow::Result<String> {
    let self_pane =
        herdr_pane_env().ok_or_else(|| anyhow::anyhow!("not running inside a herdr pane"))?;
    let split = tokio::process::Command::new("herdr")
        .args([
            "pane",
            "split",
            &self_pane,
            "--direction",
            "right",
            "--no-focus",
        ])
        .stdin(Stdio::null())
        .output()
        .await?;
    if !split.status.success() {
        anyhow::bail!(
            "herdr pane split failed: {}",
            String::from_utf8_lossy(&split.stderr).trim()
        );
    }
    let response: Value = serde_json::from_slice(&split.stdout)?;
    let pane_id = response["result"]["pane"]["pane_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("herdr pane split returned no pane id"))?
        .to_string();

    let command_line = std::iter::once(exe.to_string_lossy().into_owned())
        .chain(args.iter().cloned())
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    let command_line = format!(
        "cd {} && {}",
        shell_quote(&cwd.to_string_lossy()),
        command_line
    );
    let run = tokio::process::Command::new("herdr")
        .args(["pane", "run", &pane_id, &command_line])
        .stdin(Stdio::null())
        .output()
        .await?;
    if !run.status.success() {
        let _ = tokio::process::Command::new("herdr")
            .args(["pane", "close", &pane_id])
            .output()
            .await;
        anyhow::bail!(
            "herdr pane run failed: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        );
    }
    Ok(pane_id)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn spawn_headless(
    exe: &Path,
    args: &[String],
    cwd: &Path,
    log_file: &Path,
) -> anyhow::Result<(Option<u32>, tokio::process::Child)> {
    let log = std::fs::File::create(log_file)?;
    let stderr_log = log.try_clone()?;
    let mut command = tokio::process::Command::new(exe);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr_log)
        // The child must not report herdr agent state against the parent's
        // pane, and must itself fall back to headless spawning.
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_PANE_ID");
    let child = command.spawn()?;
    Ok((child.id(), child))
}

fn snapshot_json(snapshot: &SubagentSnapshot) -> Value {
    let mut value = json!({
        "id": snapshot.id,
        "preset": snapshot.preset,
        "state": snapshot.status.state.as_str(),
        "background": snapshot.background,
        "elapsed_s": snapshot.elapsed.as_secs(),
        "turns": snapshot.status.turns,
        "input_tokens": snapshot.status.input_tokens,
        "output_tokens": snapshot.status.output_tokens,
    });
    let object = value.as_object_mut().expect("snapshot json object");
    match &snapshot.display {
        SpawnDisplay::Pane(pane) => {
            object.insert("pane_id".into(), json!(pane));
        }
        SpawnDisplay::Log(path) => {
            object.insert("log_file".into(), json!(path.to_string_lossy()));
        }
    }
    if let Some(activity) = &snapshot.status.last_activity {
        object.insert("last_activity".into(), json!(activity));
    }
    if let Some(text) = &snapshot.status.last_text {
        object.insert("last_text".into(), json!(text));
    }
    if let Some(error) = &snapshot.status.error {
        object.insert("error".into(), json!(error));
    }
    value
}

fn finished_summary(snapshot: &SubagentSnapshot) -> String {
    let mut summary = format!(
        "subagent {} (preset {}) finished: state={}, turns={}, tokens in/out {}/{}",
        snapshot.id,
        snapshot.preset,
        snapshot.status.state.as_str(),
        snapshot.status.turns,
        snapshot.status.input_tokens,
        snapshot.status.output_tokens,
    );
    if let Some(error) = &snapshot.status.error {
        summary.push_str(&format!("\nerror: {error}"));
    }
    match &snapshot.status.result {
        Some(result) if !result.is_empty() => {
            summary.push_str("\n\n");
            summary.push_str(&truncate(result.clone(), RESULT_EXCERPT_BYTES));
        }
        _ => summary.push_str("\n(no result text)"),
    }
    summary
}

pub fn notification_prompts(notification: &SubagentNotification) -> (String, String) {
    let snapshot = &notification.snapshot;
    let model = format!(
        "[subagent notification] Background {}\n\nThis is an automated notification, not a user message. Fold the result into your ongoing work; use the agents tool for details.",
        finished_summary(snapshot)
    );
    let display = format!(
        "subagent {} ({}) finished — {}",
        snapshot.id,
        snapshot.preset,
        snapshot.status.state.as_str()
    );
    (model, display)
}

pub struct AgentTool {
    manager: SubagentManager,
    preset_summaries: Vec<(String, String)>,
}

impl AgentTool {
    pub fn new(manager: SubagentManager, cwd: &Path) -> Self {
        let preset_summaries = subagent::discover(cwd)
            .into_iter()
            .map(|preset| (preset.name, preset.description))
            .collect();
        Self {
            manager,
            preset_summaries,
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
        ToolSpec {
            name: "agent".into(),
            description: format!(
                "Spawn a subagent from a configured preset to work on a task in a separate rho process. Inside herdr the subagent runs in a visible pane; results always return here, so never read its pane or log output yourself. Blocking by default; set background=true to keep working and get notified when it finishes (check or stop it with the agents tool).\n\nPresets:\n{summaries}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "preset": {
                        "type": "string",
                        "enum": names,
                        "description": "Configured subagent preset to run"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Task for the subagent. Include all needed context; it starts fresh."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Return immediately with an id instead of waiting for the result"
                    }
                },
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
        let preset = subagent::find(&ctx.cwd, &args.preset)
            .map_err(|error| ToolError::Message(error.to_string()))?;
        let (agent_id, display) = self
            .manager
            .spawn(&preset, &args.prompt, args.background, &ctx.cwd)
            .await
            .map_err(|error| ToolError::Message(format!("failed to spawn subagent: {error}")))?;

        let where_hint = match &display {
            SpawnDisplay::Pane(pane) => format!("running in herdr pane {pane}"),
            SpawnDisplay::Log(path) => format!("running headless, log: {}", path.display()),
        };
        if args.background {
            return Ok(ToolResult {
                id,
                ok: true,
                content: format!(
                    "started background subagent {agent_id} (preset {}), {where_hint}. You will be notified when it finishes; use the agents tool to check status or stop it. Do not read its pane or log output.",
                    preset.name
                ),
            });
        }

        on_update(vec![format!("subagent {agent_id} {where_hint}")]);
        let snapshot = self
            .manager
            .wait_done(&agent_id)
            .await
            .ok_or_else(|| ToolError::Message(format!("subagent '{agent_id}' disappeared")))?;
        Ok(ToolResult {
            id,
            ok: snapshot.status.state == RunState::Ok,
            content: format!("{} ({where_hint})", finished_summary(&snapshot)),
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
                if !background {
                    let manager = self.manager.clone();
                    tokio::spawn(async move {
                        let running: Vec<String> = manager
                            .list()
                            .into_iter()
                            .filter(|snapshot| !snapshot.done && !snapshot.background)
                            .map(|snapshot| snapshot.id)
                            .collect();
                        for id in running {
                            let _ = manager.stop(&id).await;
                        }
                    });
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
            description: "Manage running subagents spawned with the agent tool: list them, check one's status and activity, or stop one (graceful cancel, then kill after a few seconds).".into(),
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
                    "no subagents have been spawned this session".to_string()
                } else {
                    let list: Vec<Value> = agents.iter().map(snapshot_json).collect();
                    serde_json::to_string_pretty(&Value::Array(list))?
                }
            }
            "status" => {
                let id = required_id(&args)?;
                let snapshot = self
                    .manager
                    .status(id)
                    .ok_or_else(|| ToolError::Message(format!("unknown subagent '{id}'")))?;
                let mut value = snapshot_json(&snapshot);
                if snapshot.done {
                    if let Some(result) = &snapshot.status.result {
                        value.as_object_mut().expect("snapshot json object").insert(
                            "result".into(),
                            json!(truncate(result.clone(), RESULT_EXCERPT_BYTES)),
                        );
                    }
                }
                serde_json::to_string_pretty(&value)?
            }
            "stop" => {
                let id = required_id(&args)?;
                let snapshot = self
                    .manager
                    .stop(id)
                    .await
                    .map_err(|error| ToolError::Message(error.to_string()))?;
                finished_summary(&snapshot)
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
