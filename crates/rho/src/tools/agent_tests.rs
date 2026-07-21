use super::*;
use crate::{
    app::agent_executor::AgentExecutor, config::Config, diagnostics::test_diagnostics,
    tools::agent_output::MODEL_NOTIFICATION_BYTES,
};

fn manager(root: &Path) -> SubagentManager {
    SubagentManager::new(AgentExecutor::new(
        Config::default(),
        root.join("rho.toml"),
        root.to_path_buf(),
    ))
}

#[test]
fn agent_tool_uses_agent_id_terminology() {
    let root = tempfile::tempdir().unwrap();
    let tool = AgentTool::new(
        manager(root.path()),
        root.path(),
        BackgroundSubagents::Enabled,
    );
    let spec = tool.spec();
    let properties = &spec.input_schema["properties"];
    assert!(properties.get("agent_id").is_some());
    assert_eq!(
        spec.input_schema["required"],
        serde_json::json!(["agent_id", "prompt"])
    );
}

#[test]
fn delegated_manager_starts_empty() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager(root.path());
    assert!(manager.list().is_empty());
    assert!(manager.status("missing").is_none());
    assert!(!manager.has_running_for_session("session-1"));
}

#[tokio::test]
async fn stopping_unknown_run_is_actionable() {
    let root = tempfile::tempdir().unwrap();
    let error = manager(root.path()).stop("missing").await.unwrap_err();
    assert!(error.to_string().contains("unknown delegated run"));
}

#[tokio::test]
async fn background_start_receipt_is_the_registration() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager(root.path());
    let tool = AgentTool::new(manager.clone(), root.path(), BackgroundSubagents::Enabled);
    let result = tool
        .call(
            serde_json::json!({
                "agent_id": "default",
                "prompt": "background task",
                "background": true,
            }),
            ToolContext {
                cwd: root.path().to_path_buf(),
                max_output_bytes: 16 * 1024,
            },
            "call-1".into(),
        )
        .await
        .unwrap();
    // The start receipt reports registration only: no live run state, no
    // activity lines, nothing that depends on how far the spawned task got.
    let runs = manager.list();
    assert_eq!(runs.len(), 1);
    let run_id = &runs[0].id;
    assert!(result.ok);
    assert_eq!(
        result.content,
        format!("agent {run_id} (default) started in background\nattach: rho attach {run_id}")
    );
}

#[test]
fn background_guidance_is_gated_by_capability() {
    let root = tempfile::tempdir().unwrap();
    let enabled = AgentTool::new(
        manager(root.path()),
        root.path(),
        BackgroundSubagents::Enabled,
    );
    let disabled = AgentTool::new(
        manager(root.path()),
        root.path(),
        BackgroundSubagents::Disabled,
    );
    assert!(enabled
        .spec()
        .description
        .contains("delivered automatically"));
    let disabled_spec = disabled.spec();
    assert!(!disabled_spec.description.contains("background"));
    assert!(disabled_spec.input_schema["properties"]
        .get("background")
        .is_none());
}

fn notification(id: &str, agent_id: &str, state: RunState) -> SubagentNotification {
    SubagentNotification {
        snapshot: SubagentSnapshot {
            id: id.into(),
            agent_id: agent_id.into(),
            background: true,
            elapsed: Duration::from_secs(5),
            status: crate::subagent::RunStatus {
                state,
                turns: 1,
                input_tokens: 10,
                output_tokens: 2,
                result: Some(format!("{id} result")),
                ..crate::subagent::RunStatus::default()
            },
            done: true,
        },
    }
}

#[test]
fn notification_prompts_batch_terminal_runs_into_one_message() {
    let first = notification("aaa111", "worker", RunState::Ok);
    let mut second = notification("bbb222", "reviewer", RunState::Stopped);
    second.snapshot.status.error = Some("review stopped before completion".into());
    second.snapshot.status.attachment_error = Some("log unavailable".into());
    let notifications = vec![first, second];
    let (model, display) = notification_prompts(&notifications);
    assert_eq!(model.matches("[agent notification]").count(), 1);
    assert!(model.contains("agent aaa111 (worker): ok"));
    assert!(model.contains("aaa111 result"));
    assert!(model.contains("agent bbb222 (reviewer): stopped"));
    assert!(model.contains("error: review stopped before completion"));
    assert!(model.contains("attachment error: log unavailable"));
    assert!(model.contains("treat its work as unverified"));
    assert_eq!(
        display,
        "agent aaa111 (worker) finished - ok\nagent bbb222 (reviewer) finished - stopped"
    );
}

#[test]
fn notification_prompts_bound_many_large_utf8_results_and_keep_run_statuses() {
    let notifications = (0..96)
        .map(|index| {
            let id = format!("run{index:03}");
            let mut notification = notification(&id, "worker", RunState::Ok);
            notification.snapshot.status.result = Some("🦀".repeat(12 * 1024));
            notification
        })
        .collect::<Vec<_>>();

    let (model, _) = notification_prompts(&notifications);

    assert!(
        model.len() <= MODEL_NOTIFICATION_BYTES,
        "{}-byte notification exceeded the {}-byte budget",
        model.len(),
        MODEL_NOTIFICATION_BYTES
    );
    for index in 0..notifications.len() {
        assert!(
            model.contains(&format!("agent run{index:03} (worker): ok")),
            "missing status for run {index}"
        );
    }
    assert!(model.contains("Any omitted or truncated result details remain available"));
    assert!(model.contains("`agents status`"));
    assert!(model.contains("`rho attach <run-id>`"));
    assert_eq!(model, notification_prompts(&notifications).0);

    let newer = (0..96)
        .map(|index| {
            let id = format!("new{index:03}");
            let mut notification = notification(&id, "reviewer", RunState::Ok);
            notification.snapshot.status.result = Some("🦀".repeat(12 * 1024));
            notification
        })
        .collect::<Vec<_>>();
    let newer = notification_prompts(&newer).0;
    let retried_context = merge_notification_context(Some(&model), &newer);
    assert!(retried_context.len() <= NOTIFICATION_CONTEXT_BYTES);
    assert!(retried_context.contains("agent new000 (reviewer): ok"));
}

async fn spawn_background_run(manager: &SubagentManager, root: &Path) -> String {
    let tool = AgentTool::new(manager.clone(), root, BackgroundSubagents::Enabled);
    tool.call(
        serde_json::json!({
            "agent_id": "default",
            "prompt": "background task",
            "background": true,
        }),
        ToolContext {
            cwd: root.to_path_buf(),
            max_output_bytes: 16 * 1024,
        },
        "call".into(),
    )
    .await
    .unwrap();
    manager.list().last().unwrap().id.clone()
}

#[tokio::test]
async fn running_queries_are_scoped_to_the_parent_session() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager(root.path());
    manager.set_session("session-1".into());
    let id = spawn_background_run(&manager, root.path()).await;

    assert!(!manager.has_running_for_session("session-2"));

    manager.stop(&id).await.unwrap();
}

#[tokio::test]
async fn observed_terminal_run_is_not_redelivered() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager(root.path());
    manager.set_session("session-1".into());
    let id = spawn_background_run(&manager, root.path()).await;
    let snapshot = manager.wait_done(&id).await.unwrap();
    assert!(snapshot.done);
    // Reading the terminal snapshot counts as delivery.
    let observed = manager.observe(&id).unwrap();
    assert!(observed.done);
    assert!(manager.take_notifications("session-1").is_empty());
    assert!(!manager.has_active_or_pending_notification("session-1"));
}

#[tokio::test]
async fn unobserved_terminal_runs_drain_as_one_batch() {
    let root = tempfile::tempdir().unwrap();
    let manager = manager(root.path());
    manager.set_session("session-1".into());
    let first = spawn_background_run(&manager, root.path()).await;
    let second = spawn_background_run(&manager, root.path()).await;
    manager.wait_done(&first).await.unwrap();
    manager.wait_done(&second).await.unwrap();
    let batch = manager.take_notifications("session-1");
    let ids = batch
        .iter()
        .map(|notification| notification.snapshot.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![first, second], "batch drains in launch order");
    assert!(
        manager.take_notifications("session-1").is_empty(),
        "a drained batch is observed and never redelivered"
    );
}

#[test]
fn lifecycle_tool_schema_is_stable() {
    let root = tempfile::tempdir().unwrap();
    let tool = AgentsTool::new(manager(root.path()));
    let spec = tool.spec();
    assert_eq!(spec.name, "agents");
    assert_eq!(
        spec.input_schema["properties"]["action"]["enum"],
        serde_json::json!(["list", "status", "stop"])
    );
    let _ = test_diagnostics("test", "test");
}
