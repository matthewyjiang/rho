use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::subagent::write_status;

fn test_entry(preset: &str, background: bool) -> AgentEntry {
    AgentEntry {
        preset: preset.into(),
        background,
        started: Instant::now(),
        display: SpawnDisplay::Log(PathBuf::from("/tmp/log.txt")),
        on_exit: OnExit::Keep,
        pid: None,
        status: RunStatus::default(),
        done: false,
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

    assert!(manager.has_active());
    assert!(manager.take_notifications().is_empty());

    write_status(
        &output_file,
        &RunStatus {
            state: RunState::Ok,
            pid: Some(123),
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
    assert!(!manager.has_active());

    let notifications = manager.take_notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].snapshot.id, "x1");
    // Notifications are delivered once.
    assert!(manager.take_notifications().is_empty());
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
    assert!(manager.take_notifications().is_empty());
}

#[tokio::test]
async fn stop_unknown_subagent_errors() {
    let manager = SubagentManager::new();

    let error = manager.stop("missing").await.unwrap_err();

    assert!(error.to_string().contains("unknown subagent"));
}

#[tokio::test]
async fn stop_without_pid_reports_helpfully() {
    let manager = SubagentManager::new();
    insert_entry(&manager, "x3", test_entry("explorer", false));

    let error = manager.stop("x3").await.unwrap_err();

    assert!(error.to_string().contains("has not reported a pid"));
}

#[test]
fn run_args_shape() {
    let args = run_args("explorer", Path::new("/tmp/out.json"), "find the tests");

    assert_eq!(
        args,
        vec![
            "--no-subagents",
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
fn shell_quote_escapes_single_quotes() {
    assert_eq!(shell_quote("plain"), "'plain'");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
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
    let tool = AgentTool::new(SubagentManager::new(), dir.path());

    let spec = tool.spec();

    assert_eq!(spec.name, "agent");
    assert!(spec.description.contains("explorer:"));
    assert!(spec.description.contains("worker:"));
    let names = spec.input_schema["properties"]["preset"]["enum"]
        .as_array()
        .unwrap();
    assert!(names.iter().any(|name| name == "explorer"));
    assert!(names.iter().any(|name| name == "worker"));
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
        display: SpawnDisplay::Pane("1-3".into()),
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
    assert_eq!(display, "subagent x7 (explorer) finished — ok");
}
