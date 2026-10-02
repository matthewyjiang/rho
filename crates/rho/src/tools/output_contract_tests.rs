use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{ProcessEnvironment, ScopedWorkspacePolicy, ToolHost, ToolHostCall, Workspace};
use rho_tools::{coding_tools, shell_tool, ShellToolOptions};
use serde_json::json;

use super::{
    agent::{sdk_bundle, DelegationBundleOptions, DelegationToolSelection},
    code_mode::{CodeModeSurface, CODEMODE_TOOL_NAME},
    sdk_registry::ToolBundle,
};
use crate::{
    config::Config,
    subagent::{RunState, RunStatus},
};

// Covers: combined delegated-run data obeys the configured output budget.
// Owner: delegated-agent producer output contract.
#[tokio::test]
async fn delegated_agent_list_omits_combined_data_over_configured_budget() {
    let dir = tempfile::tempdir().unwrap();
    let status = RunStatus {
        state: RunState::Ok,
        result: Some("x".repeat(16 * 1024)),
        ..RunStatus::default()
    };
    // Two excerpts fit, but the combined three-run list cannot.
    let max_output_bytes = status.result.as_ref().unwrap().len() * 2;
    let bundle = sdk_bundle(
        &Config {
            max_output_bytes,
            ..Config::default()
        },
        DelegationBundleOptions {
            cwd: dir.path().to_path_buf(),
            tools: DelegationToolSelection::Manage,
            config_path: dir.path().join("config.toml"),
            catalog: None,
        },
        Arc::new(()),
    );
    let manager = bundle.manager_handle();
    for id in ["abc001", "abc002", "abc003"] {
        manager.insert_completed_status_for_test(id, "fixture-session", status.clone());
    }
    let output = ToolHost::builder()
        .tool_shared(bundle.tools()[0].clone())
        .build()
        .unwrap()
        .invoke(ToolHostCall::new("agents", json!({"action": "list"})))
        .await
        .unwrap();
    assert_eq!(output.structured_content(), None);
    assert!(output.content().starts_with(&format!(
        "[structured data truncated: max_output_bytes {max_output_bytes},"
    )));
    manager.shutdown().await;
}

// Covers: advertised returns schemas accept real results after script conversion,
// including process stop's successful acceptance receipt and nullable shell exits.
// Owner: host tool output contracts at the codemode consumer boundary.
#[tokio::test]
async fn built_in_script_results_match_advertised_schemas() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "needle\n").unwrap();
    let manager = super::process::ProcessManager::new(Default::default());
    // Block on an external event, not a timer: stop's target cannot exit first.
    #[cfg(unix)]
    let wait_command = "mkfifo stop.pipe && cat stop.pipe";
    #[cfg(windows)]
    let wait_command = "Wait-Event";
    let stopped = manager
        .start(wait_command.into(), dir.path(), None)
        .await
        .unwrap();
    let mut tools = coding_tools(Default::default());
    tools.push(Arc::new(super::process::sdk_process::SdkProcess::new(
        super::process::Process::new(manager.clone()),
        rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
        ProcessEnvironment::InheritAll,
    )));
    tools.push(shell_tool(ShellToolOptions::default()));
    let surface = Arc::new(CodeModeSurface::default());
    surface.sync(&tools);
    let host = surface
        .orchestration_tools()
        .into_iter()
        .fold(
            ToolHost::builder()
                .workspace(Workspace::new(dir.path()).unwrap())
                .workspace_policy(
                    ScopedWorkspacePolicy::new()
                        .allow_read_paths()
                        .allow_processes(),
                ),
            |builder, tool| builder.tool_shared(tool),
        )
        .build()
        .unwrap();
    for (name, args) in [
        ("grep", json!({"pattern": "needle"})),
        ("glob", json!({"pattern": "*"})),
        ("list_dir", json!({"path": "."})),
        (
            "process",
            json!({"action": "start", "command": "echo needle"}),
        ),
        (
            "process",
            json!({"action": "stop", "process_id": stopped.process_id}),
        ),
        #[cfg(unix)]
        ("bash", json!({"command": "printf needle"})),
        #[cfg(windows)]
        ("powershell", json!({"command": "Write-Output needle"})),
    ] {
        let entry = surface
            .search(name, usize::MAX)
            .into_iter()
            .find(|entry| entry.name == name)
            .unwrap();
        let output = host
            .invoke(ToolHostCall::new(
                CODEMODE_TOOL_NAME,
                json!({"script": format!("result = call_tool({name:?}, {args})")}),
            ))
            .await
            .unwrap();
        let result = &output.structured_content().unwrap()["return_value"];
        jsonschema::validator_for(&entry.returns)
            .unwrap()
            .validate(result)
            .unwrap();
        assert_eq!(result["is_error"], false, "{name}: {result}");
        assert!(!result["data"].is_null(), "{name}: {result}");
    }
    manager.shutdown().await;
}
