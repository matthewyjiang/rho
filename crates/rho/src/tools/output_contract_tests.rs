use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{ProcessEnvironment, ScopedWorkspacePolicy, ToolHost, ToolHostCall, Workspace};
use rho_tools::{coding_tools, shell_tool, ShellToolOptions};
use serde_json::json;

use super::code_mode::{CodeModeSurface, CODEMODE_TOOL_NAME};

// Covers: completed delegated runs cannot bypass the configured output budget
// through a combined structured list, including when consumed from codemode.
// Owner: delegated-agent producer output contract
#[tokio::test]
async fn delegated_agent_list_omits_combined_data_over_configured_budget() {
    use super::sdk_registry::ToolBundle;
    use crate::{
        config::Config,
        subagent::{RunState, RunStatus},
    };

    let dir = tempfile::tempdir().unwrap();
    let result = "x".repeat(16 * 1024);
    // Two excerpts fit in this fixture budget, but the three-run list cannot.
    let max_output_bytes = result.len() * 2;
    let config = Config {
        max_output_bytes,
        ..Config::default()
    };
    let bundle = super::agent::sdk_bundle(
        &config,
        super::agent::DelegationBundleOptions {
            cwd: dir.path().to_path_buf(),
            tools: super::agent::DelegationToolSelection::Manage,
            config_path: dir.path().join("config.toml"),
            catalog: None,
        },
        Arc::new(()),
    );
    let manager = bundle.manager_handle();
    for id in ["abc001", "abc002", "abc003"] {
        manager.insert_completed_status_for_test(
            id,
            "fixture-session",
            RunStatus {
                state: RunState::Ok,
                result: Some(result.clone()),
                ..RunStatus::default()
            },
        );
    }
    let baseline_host = ToolHost::builder()
        .tool(super::agent::AgentsTool::new(manager.clone()).with_max_output_bytes(usize::MAX))
        .build()
        .unwrap();
    let baseline = baseline_host
        .invoke(ToolHostCall::new("agents", json!({"action": "list"})))
        .await
        .unwrap();
    let data = baseline.structured_content().unwrap();
    assert_eq!(data["runs"].as_array().unwrap().len(), 3);
    let received = serde_json::to_vec(data).unwrap().len();
    assert!(received > max_output_bytes);
    for run in data["runs"].as_array().unwrap() {
        assert!(serde_json::to_vec(run).unwrap().len() < max_output_bytes);
    }

    let surface = Arc::new(CodeModeSurface::default());
    surface.sync(bundle.tools());
    let host = surface
        .orchestration_tools()
        .into_iter()
        .fold(ToolHost::builder(), |builder, tool| {
            builder.tool_shared(tool)
        })
        .build()
        .unwrap();
    let output = host
        .invoke(ToolHostCall::new(
            CODEMODE_TOOL_NAME,
            json!({"script": "result = call_tool(\"agents\", {\"action\": \"list\"})"}),
        ))
        .await
        .unwrap();
    let envelope = &output.structured_content().unwrap()["return_value"];
    assert_eq!(envelope["is_error"], false);
    assert!(envelope["data"].is_null());
    let content = envelope["content"].as_str().unwrap();
    assert_eq!(
        content.lines().next().unwrap(),
        format!(
            "[structured data truncated: max_output_bytes {max_output_bytes}, received {received} bytes]"
        )
    );
    assert!(content.len() <= max_output_bytes);
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
