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
