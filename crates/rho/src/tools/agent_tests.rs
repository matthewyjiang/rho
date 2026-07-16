use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::subagent::{self, write_status};

fn test_entry(preset: &str, background: bool) -> AgentEntry {
    let (force_kill, _requests) = tokio::sync::mpsc::unbounded_channel();
    AgentEntry {
        preset: preset.into(),
        background,
        started: Instant::now(),
        log_file: PathBuf::from("/tmp/log.txt"),
        output_file: PathBuf::from("result.json"),
        force_kill,
        session_id: Some("session-a".into()),
        status: RunStatus::default(),
        done: false,
        process_exited: false,
        notified: false,
    }
}

fn insert_entry(manager: &SubagentManager, id: &str, entry: AgentEntry) {
    manager
        .inner
        .lock()
        .expect("subagent registry lock")
        .insert(id.into(), entry);
}

#[tokio::test]
async fn watcher_detects_terminal_status_and_notifies() {
    let dir = TempDir::new().unwrap();
    let output_file = dir.path().join("result.json");
    let manager = SubagentManager::new();
    insert_entry(&manager, "x1", test_entry("explorer", true));
    manager.watch_status_file("x1", output_file.clone());

    assert!(manager.has_active_or_pending_notification("session-a"));
    assert!(manager.take_notifications("session-a").is_empty());

    write_status(
        &output_file,
        &RunStatus {
            state: RunState::Ok,
            result: Some("all done".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();

    let snapshot = tokio::time::timeout(Duration::from_secs(5), manager.wait_done("x1"))
        .await
        .expect("watcher should observe the terminal state")
        .expect("entry exists");
    assert_eq!(snapshot.status.state, RunState::Ok);
    assert_eq!(snapshot.status.result.as_deref(), Some("all done"));
    assert!(manager.has_active_or_pending_notification("session-a"));

    let notifications = manager.take_notifications("session-a");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].snapshot.id, "x1");
    // Notifications are delivered once.
    assert!(manager.take_notifications("session-a").is_empty());
}

#[tokio::test]
async fn blocking_agents_do_not_notify() {
    let dir = TempDir::new().unwrap();
    let output_file = dir.path().join("result.json");
    let manager = SubagentManager::new();
    insert_entry(&manager, "x2", test_entry("worker", false));
    manager.watch_status_file("x2", output_file.clone());

    write_status(
        &output_file,
        &RunStatus {
            state: RunState::Error,
            error: Some("boom".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();

    tokio::time::timeout(Duration::from_secs(5), manager.wait_done("x2"))
        .await
        .unwrap()
        .unwrap();
    assert!(manager.take_notifications("session-a").is_empty());
}

#[tokio::test]
async fn stop_unknown_subagent_errors() {
    let manager = SubagentManager::new();

    let error = manager.stop("missing").await.unwrap_err();

    assert!(error.to_string().contains("unknown subagent"));
}

#[tokio::test]
async fn stop_waits_for_graceful_process_exit() {
    let dir = TempDir::new().unwrap();
    let output_file = dir.path().join(subagent::RESULT_FILE_NAME);
    let cancel_file = subagent::cancel_file_for(&output_file);
    let manager = SubagentManager::new();
    let mut entry = test_entry("explorer", false);
    entry.output_file = output_file.clone();
    insert_entry(&manager, "x3", entry);
    manager.watch_status_file("x3", output_file.clone());

    let completion_manager = manager.clone();
    let completion = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !cancel_file.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancel marker was not written");
        write_status(
            &output_file,
            &RunStatus {
                state: RunState::Stopped,
                result: Some("partial result".into()),
                ..RunStatus::default()
            },
        )
        .unwrap();
        let status = subagent::read_status(&output_file).expect("terminal status");
        let mut agents = completion_manager
            .inner
            .lock()
            .expect("subagent registry lock");
        let entry = agents.get_mut("x3").expect("subagent entry");
        entry.status = status;
        entry.done = true;
        entry.process_exited = true;
    });

    let snapshot = manager.stop("x3").await.unwrap();
    completion.await.unwrap();

    assert_eq!(snapshot.status.state, RunState::Stopped);
    assert_eq!(snapshot.status.result.as_deref(), Some("partial result"));
}

#[cfg(unix)]
#[tokio::test]
async fn spawned_child_uses_a_distinct_process_group() {
    let directory = TempDir::new().unwrap();
    let log_file = directory.path().join("log.txt");
    let mut child = spawn_headless(
        Path::new("/bin/sleep"),
        &["30".into()],
        directory.path(),
        &log_file,
    )
    .unwrap();
    let pid = child.id().expect("spawned child") as libc::pid_t;

    assert_ne!(unsafe { libc::getpgid(pid) }, unsafe { libc::getpgrp() });

    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_kills_a_child_that_reported_completion_before_exiting() {
    let directory = TempDir::new().unwrap();
    let output_file = directory.path().join(subagent::RESULT_FILE_NAME);
    let log_file = directory.path().join(subagent::LOG_FILE_NAME);
    let child = spawn_headless(
        Path::new("/bin/sleep"),
        &["30".into()],
        directory.path(),
        &log_file,
    )
    .unwrap();
    let pid = child.id().expect("spawned child") as libc::pid_t;
    let manager = SubagentManager::new();
    let force_kill = manager.watch_child("x4", child);
    let mut entry = test_entry("explorer", true);
    entry.output_file = output_file;
    entry.force_kill = force_kill;
    entry.status.state = RunState::Ok;
    entry.done = true;
    insert_entry(&manager, "x4", entry);

    tokio::time::timeout(Duration::from_secs(5), manager.shutdown())
        .await
        .expect("shutdown should reap the completed child");

    let entry = manager.inner.lock().unwrap();
    assert!(entry["x4"].process_exited);
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[cfg(unix)]
#[tokio::test]
async fn child_failure_without_status_is_persisted_for_attach() {
    let directory = TempDir::new().unwrap();
    let output_file = directory.path().join(subagent::RESULT_FILE_NAME);
    let log_file = directory.path().join(subagent::LOG_FILE_NAME);
    let child = spawn_headless(Path::new("true"), &[], directory.path(), &log_file).unwrap();
    let manager = SubagentManager::new();
    let force_kill = manager.watch_child("x5", child);
    let mut entry = test_entry("explorer", true);
    entry.output_file = output_file.clone();
    entry.force_kill = force_kill;
    insert_entry(&manager, "x5", entry);

    let snapshot = tokio::time::timeout(Duration::from_secs(5), manager.wait_done("x5"))
        .await
        .expect("child watcher should finish")
        .expect("subagent entry");
    let persisted = subagent::read_status(&output_file).expect("persisted terminal status");

    assert_eq!(persisted, snapshot.status);
    assert_eq!(persisted.state, RunState::Error);
    assert!(persisted
        .error
        .as_deref()
        .is_some_and(|error| error.contains("without writing a result")));
}

#[test]
fn run_args_shape() {
    let args = run_args(
        "explorer",
        Path::new("/tmp/out.json"),
        "find the tests",
        Some(Path::new("/tmp/rho.toml")),
    );

    assert_eq!(
        args,
        vec![
            "--no-subagents",
            "--config",
            "/tmp/rho.toml",
            "run",
            "--preset",
            "explorer",
            "--output-file",
            "/tmp/out.json",
            "--",
            "find the tests",
        ]
    );
}

#[test]
fn agent_ids_are_short_and_unique() {
    let a = new_agent_id();
    let b = new_agent_id();

    assert_eq!(a.len(), 6);
    assert_ne!(a, b);
}

#[test]
fn agent_tool_spec_lists_presets() {
    let dir = TempDir::new().unwrap();
    let tool = AgentTool::new(
        SubagentManager::new(),
        dir.path(),
        BackgroundSubagents::Enabled,
    );

    let spec = tool.spec();

    assert_eq!(spec.name, "agent");
    assert!(spec
        .description
        .starts_with("Delegate a substantial, self-contained task to a fresh agent."));
    assert!(!spec.description.contains("herdr"));
    assert!(!spec.description.contains("Blocking by default"));
    assert!(spec
        .description
        .contains("Background results start a new turn automatically"));
    assert!(spec
        .description
        .contains("Do not poll or wait when no foreground work remains"));
    assert!(spec.description.contains("rho attach <id>"));
    assert!(spec.description.contains("explorer:"));
    assert!(spec.description.contains("reviewer:"));
    assert!(spec.description.contains("worker:"));
    assert_eq!(
        spec.input_schema["properties"]["preset"]["description"],
        "Agent preset"
    );
    assert_eq!(
        spec.input_schema["properties"]["prompt"]["description"],
        "Self-contained task and all context the agent needs"
    );
    assert_eq!(
        spec.input_schema["properties"]["background"]["description"],
        "Run concurrently and return an id immediately"
    );
    let names = spec.input_schema["properties"]["preset"]["enum"]
        .as_array()
        .unwrap();
    assert!(names.iter().any(|name| name == "explorer"));
    assert!(names.iter().any(|name| name == "reviewer"));
    assert!(names.iter().any(|name| name == "worker"));
}

#[test]
fn agents_tool_spec_is_concise() {
    let spec = AgentsTool::new(SubagentManager::new()).spec();

    assert_eq!(spec.description, "Check or stop background subagents.");
}

#[test]
fn notifications_are_isolated_by_session() {
    let manager = SubagentManager::new();
    let mut entry = test_entry("explorer", true);
    entry.done = true;
    insert_entry(&manager, "old", entry);

    assert!(manager.take_notifications("session-b").is_empty());
    assert!(manager.has_active_or_pending_notification("session-a"));
    assert_eq!(manager.take_notifications("session-a").len(), 1);
}

#[test]
fn headless_agent_schema_does_not_advertise_background_runs() {
    let dir = TempDir::new().unwrap();
    let tool = AgentTool::new(
        SubagentManager::new(),
        dir.path(),
        BackgroundSubagents::Disabled,
    );

    let spec = tool.spec();

    assert!(spec.input_schema["properties"].get("background").is_none());
    assert!(!spec.description.contains("background=true"));
}

#[tokio::test]
async fn agents_tool_lists_and_reports_status() {
    let dir = TempDir::new().unwrap();
    let manager = SubagentManager::new();
    let mut entry = test_entry("explorer", true);
    entry.status.state = RunState::Running;
    entry.status.turns = 4;
    entry.status.last_activity = Some("tool: bash".into());
    insert_entry(&manager, "x9", entry);
    let tool = AgentsTool::new(manager);
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 65536,
    };

    let list = tool
        .call(
            serde_json::json!({"action": "list"}),
            ctx.clone(),
            "1".into(),
        )
        .await
        .unwrap();
    assert!(list.content.contains("\"id\": \"x9\""));
    assert!(list.content.contains("\"state\": \"running\""));

    let status = tool
        .call(
            serde_json::json!({"action": "status", "id": "x9"}),
            ctx.clone(),
            "2".into(),
        )
        .await
        .unwrap();
    assert!(status.content.contains("\"turns\": 4"));
    assert!(status.content.contains("tool: bash"));

    let missing = tool
        .call(
            serde_json::json!({"action": "status", "id": "nope"}),
            ctx.clone(),
            "3".into(),
        )
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("unknown subagent"));

    let no_id = tool
        .call(serde_json::json!({"action": "stop"}), ctx, "4".into())
        .await
        .unwrap_err();
    assert!(no_id.to_string().contains("requires a subagent id"));
}

#[test]
fn notification_prompts_summarize_result() {
    let snapshot = SubagentSnapshot {
        id: "x7".into(),
        preset: "explorer".into(),
        background: true,
        elapsed: Duration::from_secs(90),
        log_file: PathBuf::from("/tmp/log.txt"),
        status: RunStatus {
            state: RunState::Ok,
            turns: 6,
            result: Some("the answer".into()),
            ..RunStatus::default()
        },
        done: true,
    };

    let (model, display) = notification_prompts(&SubagentNotification { snapshot });

    assert!(model.contains("subagent x7"));
    assert!(model.contains("the answer"));
    assert!(model.contains("automated notification"));
    assert_eq!(display, "subagent x7 (explorer) finished - ok");
}
