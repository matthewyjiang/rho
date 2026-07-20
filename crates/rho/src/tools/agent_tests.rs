use super::*;
use crate::{app::agent_executor::AgentExecutor, config::Config, diagnostics::test_diagnostics};

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
}

#[tokio::test]
async fn stopping_unknown_run_is_actionable() {
    let root = tempfile::tempdir().unwrap();
    let error = manager(root.path()).stop("missing").await.unwrap_err();
    assert!(error.to_string().contains("unknown delegated run"));
}

#[tokio::test]
async fn background_start_resolves_without_waiting_for_activity() {
    let root = tempfile::tempdir().unwrap();
    let tool = AgentTool::new(
        manager(root.path()),
        root.path(),
        BackgroundSubagents::Enabled,
    );
    let started = Instant::now();
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
    // The start receipt is registration: the result must resolve immediately
    // instead of polling for the run's first activity.
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(result.ok);
    assert!(result.content.contains("started in background"));
    assert!(result.content.contains("state: "));
    assert!(result.content.contains("attach: rho attach"));
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
