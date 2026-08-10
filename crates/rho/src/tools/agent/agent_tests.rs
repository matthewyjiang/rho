use std::{ffi::OsString, num::NonZeroUsize, sync::MutexGuard};

use pretty_assertions::assert_eq;
use rho_sdk::{
    tool::{
        tool_progress_channel, Tool, ToolAccessMode, ToolContext, ToolExecutionPolicy,
        ToolInvocation, ToolPreparationContext, ToolResourceKind,
    },
    CancellationToken, ToolCallId, Workspace,
};

use super::*;
use crate::app::subagent_host_input::SubagentHostInputBridge;
use crate::{
    app::agent_executor::AgentExecutor, config::Config,
    tools::agent_output::MODEL_NOTIFICATION_BYTES,
};

/// Isolates delegated-run storage from other tests that mutate `RHO_HOME`.
struct IsolatedRhoHome {
    _dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
    previous: Option<OsString>,
}

impl IsolatedRhoHome {
    fn new() -> Self {
        let guard = crate::paths::process_env_lock();
        let dir = tempfile::tempdir().expect("rho home tempdir");
        let previous = std::env::var_os("RHO_HOME");
        std::env::set_var("RHO_HOME", dir.path());
        Self {
            _dir: dir,
            _guard: guard,
            previous,
        }
    }
}

impl Drop for IsolatedRhoHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("RHO_HOME", value),
            None => std::env::remove_var("RHO_HOME"),
        }
    }
}

struct ManagerFixture {
    manager: SubagentManager,
    _rho_home: IsolatedRhoHome,
}

impl ManagerFixture {
    fn new(root: &Path) -> Self {
        let rho_home = IsolatedRhoHome::new();
        Self {
            manager: SubagentManager::new(AgentExecutor::new(
                Config::default(),
                root.join("rho.toml"),
                root.to_path_buf(),
                SubagentHostInputBridge::new(),
                crate::app::subagent_notice::SubagentNoticeBridge::new(),
            )),
            _rho_home: rho_home,
        }
    }

    fn manager(&self) -> SubagentManager {
        self.manager.clone()
    }
}

fn manager(root: &Path) -> ManagerFixture {
    ManagerFixture::new(root)
}

fn invocation(arguments: serde_json::Value) -> ToolInvocation {
    ToolInvocation::new(ToolCallId::from_string("call-1").unwrap(), arguments)
}

fn tool_context(root: &Path) -> ToolContext {
    let (progress, _receiver) = tool_progress_channel(NonZeroUsize::new(4).unwrap());
    ToolContext::new(
        Some(Workspace::new(root).unwrap()),
        CancellationToken::new(),
        progress,
    )
}

async fn call_agent(tool: &AgentTool, root: &Path, arguments: serde_json::Value) -> ToolOutput {
    tool.call(invocation(arguments), tool_context(root))
        .await
        .expect("agent tool call")
}

#[tokio::test]
async fn stopping_unknown_run_is_actionable() {
    let root = tempfile::tempdir().unwrap();
    let fixture = manager(root.path());
    let error = fixture.manager().stop("abcdef").await.unwrap_err();
    assert!(error.to_string().contains("unknown delegated run"));
}

fn notification(id: &str, agent_id: &str, state: RunState) -> SubagentNotification {
    SubagentNotification {
        snapshot: SubagentSnapshot {
            id: id.into(),
            agent_id: agent_id.into(),
            elapsed: Duration::from_secs(5),
            status: crate::subagent::RunStatus {
                state,
                turns: 1,
                input_tokens: Some(10),
                output_tokens: Some(2),
                result: Some(format!("{id} result")),
                ..crate::subagent::RunStatus::default()
            },
            done: true,
        },
    }
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
    let output = call_agent(
        &tool,
        root,
        serde_json::json!({
            "agent_id": "default",
            "prompt": "background task",
            "background": true,
        }),
    )
    .await;
    // Parse the start receipt. Do not use list().last(): list sorts by elapsed,
    // so equal-duration runs make "last" non-deterministic across HashMap order.
    let content = output.content();
    let id = content
        .strip_prefix("agent ")
        .and_then(|rest| rest.split_once(' ').map(|(id, _)| id))
        .expect("background start receipt names the run id");
    assert_eq!(id.len(), 6, "run id should be 6 hex chars: {content}");
    id.to_string()
}

#[tokio::test]
async fn running_queries_are_scoped_to_the_parent_session() {
    let root = tempfile::tempdir().unwrap();
    let fixture = manager(root.path());
    let manager = fixture.manager();
    manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
        "session-1",
        None,
    ));
    let id = spawn_background_run(&manager, root.path()).await;

    assert!(!manager.has_running_for_session("session-2"));

    manager.stop(&id).await.unwrap();
}

#[tokio::test]
async fn observed_terminal_run_is_not_redelivered() {
    let root = tempfile::tempdir().unwrap();
    let fixture = manager(root.path());
    let manager = fixture.manager();
    manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
        "session-1",
        None,
    ));
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
    let fixture = manager(root.path());
    let manager = fixture.manager();
    manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
        "session-1",
        None,
    ));
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
fn claim_terminal_costs_is_idempotent_and_session_scoped() {
    let root = tempfile::tempdir().unwrap();
    let fixture = manager(root.path());
    let manager = fixture.manager();
    manager.insert_completed_for_test("aaa111", "session-1", Some(0.034271));
    manager.insert_completed_for_test("bbb222", "session-1", Some(0.01));
    manager.insert_completed_for_test("ccc333", "session-2", Some(0.5));
    manager.insert_completed_for_test("ddd444", "session-1", None);

    assert_eq!(manager.claim_terminal_costs_usd_micros("session-1"), 44_271);
    assert_eq!(manager.claim_terminal_costs_usd_micros("session-1"), 0);
    assert_eq!(
        manager.claim_terminal_costs_usd_micros("session-2"),
        500_000
    );
}

fn one_access(
    prepared: &rho_sdk::tool::PreparedToolInvocation<'_>,
) -> (ToolResourceKind, ToolAccessMode) {
    let ToolExecutionPolicy::ResourceAware { accesses } = prepared.execution_policy() else {
        panic!("expected a resource-aware invocation");
    };
    assert_eq!(accesses.len(), 1);
    (accesses[0].resource().kind(), accesses[0].mode())
}

fn preparation_context(root: &Path) -> ToolPreparationContext {
    ToolPreparationContext::new(
        Some(Workspace::new(root).unwrap()),
        CancellationToken::new(),
    )
}

#[tokio::test]
async fn agent_and_agents_prepare_subagent_manager_resources() {
    let root = tempfile::tempdir().unwrap();
    let fixture = manager(root.path());
    let manager = fixture.manager();
    let agent = AgentTool::new(manager.clone(), root.path(), BackgroundSubagents::Enabled);
    let agents = AgentsTool::new(manager);

    let launch = agent
        .prepare(
            invocation(serde_json::json!({
                "agent_id": "default",
                "prompt": "task",
            })),
            preparation_context(root.path()),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&launch),
        (ToolResourceKind::ManagerState, ToolAccessMode::Shared)
    );

    let background = agent
        .prepare(
            invocation(serde_json::json!({
                "agent_id": "default",
                "prompt": "task",
                "background": true,
            })),
            preparation_context(root.path()),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&background),
        (ToolResourceKind::ManagerState, ToolAccessMode::Shared)
    );

    let list = agents
        .prepare(
            invocation(serde_json::json!({"action": "list"})),
            preparation_context(root.path()),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&list),
        (ToolResourceKind::ManagerState, ToolAccessMode::Shared)
    );

    let status = agents
        .prepare(
            invocation(serde_json::json!({"action": "status", "id": "run-1"})),
            preparation_context(root.path()),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&status),
        (ToolResourceKind::ManagerState, ToolAccessMode::Shared)
    );

    let stop = agents
        .prepare(
            invocation(serde_json::json!({"action": "stop", "id": "run-1"})),
            preparation_context(root.path()),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&stop),
        (ToolResourceKind::ManagerState, ToolAccessMode::Shared)
    );
}

#[tokio::test]
async fn concurrent_background_launches_register_together() {
    let root = tempfile::tempdir().unwrap();
    let fixture = manager(root.path());
    let manager = fixture.manager();
    let tool = AgentTool::new(manager.clone(), root.path(), BackgroundSubagents::Enabled);
    let first = call_agent(
        &tool,
        root.path(),
        serde_json::json!({
            "agent_id": "default",
            "prompt": "first background task",
            "background": true,
        }),
    );
    let second = call_agent(
        &tool,
        root.path(),
        serde_json::json!({
            "agent_id": "default",
            "prompt": "second background task",
            "background": true,
        }),
    );
    let (first, second) = tokio::join!(first, second);
    let runs = manager.list();
    assert_eq!(runs.len(), 2, "both background launches should register");
    let ids = runs.iter().map(|run| run.id.as_str()).collect::<Vec<_>>();
    assert!(ids.iter().any(|id| first.content().contains(id)));
    assert!(ids.iter().any(|id| second.content().contains(id)));
}
