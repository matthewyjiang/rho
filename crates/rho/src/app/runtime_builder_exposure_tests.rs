//! Provider-boundary coverage of live app advertisement and script routing.

use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    SessionOptions, SystemPrompt, Workspace,
};
use serde_json::json;

use super::{build_runtime, RuntimeBuildOptions};
use crate::{
    app::policy::AppPolicy,
    compaction::CompactionConfig,
    config::{CodemodeMode, Config},
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    tools::{
        code_mode::{CODEMODE_TOOL_NAME, TOOL_SEARCH_NAME},
        sdk_registry::{AppToolSet, StaticToolBundle, ToolSetOptions},
    },
};

fn runtime_for(
    config: &Config,
    tools: &AppToolSet,
    provider: &ScriptedProvider,
    workspace: Workspace,
) -> rho_sdk::Rho {
    build_runtime(RuntimeBuildOptions {
        provider: Arc::new(provider.clone()),
        tools,
        workspace,
        workspace_policy: AppPolicy::for_mode(PermissionMode::Bypass, Default::default()),
        approval_session: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        service_tier: None,
        compaction: CompactionConfig::from(config),
        context_window: None,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        hooks: None,
        diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
        recall: None,
    })
    .unwrap()
}

fn text_turn() -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        "done".into(),
    )]))
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        },
    )]))
}

// Covers: modes and descriptions are resolved for every request of ONE live
// runtime; changing mode must neither rebuild it nor leak MCP schemas.
// Owner: app runtime builder + SDK visibility snapshot.
#[tokio::test]
async fn codemode_mode_selects_each_live_provider_request() {
    use CodemodeMode::{On, Only};
    let config = Config::default();
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.add_bundle(StaticToolBundle::new(vec![Arc::new(ScriptedTool::new(
        ToolSpec {
            name: "mcp__docs__lookup".into(),
            description: "documentation".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("docs")),
    ))]));
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [text_turn(), text_turn(), text_turn()],
    );
    let runtime = runtime_for(
        &config,
        &tools,
        &provider,
        Workspace::new(std::env::current_dir().unwrap()).unwrap(),
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let watched = [
        CODEMODE_TOOL_NAME,
        TOOL_SEARCH_NAME,
        "read_file",
        "write",
        "mcp__docs__lookup",
    ];
    let cases = [
        (On, [true, true, true, true, false]),
        (Only, [true, true, false, false, false]),
        (On, [true, true, true, true, false]),
    ];
    for (index, (mode, expected)) in cases.into_iter().enumerate() {
        tools.code_mode().set_mode(mode);
        session.complete("hi").await.unwrap();
        let requests = provider.recorded_requests();
        let declared = &requests[index].tools;
        assert_eq!(
            watched.map(|name| declared.iter().any(|spec| spec.name == name)),
            expected
        );
        if mode == On {
            let original = tools
                .tools()
                .iter()
                .find(|tool| tool.spec().name == "read_file")
                .unwrap()
                .spec();
            let described = tools.tool_visibility().describe(&original).unwrap();
            assert_eq!(
                declared
                    .iter()
                    .find(|spec| spec.name == "read_file")
                    .unwrap()
                    .description,
                described
            );
        }
        let original = tools
            .tools()
            .iter()
            .find(|tool| tool.spec().name == CODEMODE_TOOL_NAME)
            .unwrap()
            .spec();
        assert_eq!(
            declared
                .iter()
                .find(|spec| spec.name == CODEMODE_TOOL_NAME)
                .unwrap(),
            &original
        );
    }
}

// Covers: only mode blocks direct model execution of unadvertised natives but
// still permits nested calls under the same session authorization.
// Owner: application runtime tool exposure.
#[tokio::test]
async fn only_mode_routes_natives_through_codemode() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.code_mode().set_mode(CodemodeMode::Only);
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            tool_call(
                "direct",
                "write",
                json!({"path": "direct.txt", "content": "direct"}),
            ),
            tool_call(
                "nested",
                CODEMODE_TOOL_NAME,
                json!({"script": r#"result = call_tool("write", {"path": "nested.txt", "content": "nested"})["content"]"#}),
            ),
            text_turn(),
        ],
    );
    let runtime = runtime_for(
        &config,
        &tools,
        &provider,
        Workspace::new(root.path()).unwrap(),
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    session.complete("write files").await.unwrap();
    assert_eq!(
        (
            root.path().join("direct.txt").exists(),
            std::fs::read_to_string(root.path().join("nested.txt")).ok()
        ),
        (false, Some("nested".into()))
    );
}
