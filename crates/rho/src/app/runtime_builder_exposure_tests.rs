//! Provider-boundary coverage of live app advertisement and script routing.

use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
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
// runtime; changing mode must neither rebuild it nor leak unsearched MCP schemas.
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

// Covers: top-level search loads only returned MCP hits for direct calls on
// the next request, while script discovery and execution leave them deferred.
// Owner: app discovery + provider advertisement + SDK dispatch.
#[tokio::test]
async fn tool_search_loads_mcp_hits_for_direct_calls() {
    use crate::tools::mcp::exported_name::namespaced_tool_name;

    for mode in [CodemodeMode::On, CodemodeMode::Only] {
        let config = Config::default();
        let mut tools = AppToolSet::new(
            &config,
            RuntimeDiagnostics::new(&config),
            ToolSetOptions::default(),
        );
        tools.code_mode().set_mode(mode);
        let found = namespaced_tool_name("t3-code", "unwatch_pull_request");
        let deferred = namespaced_tool_name("t3-code", "watch_pull_request");
        let parameters = json!({
            "type": "object",
            "properties": {"threadId": {"type": "string"}},
            "required": ["threadId"],
            "additionalProperties": false,
        });
        tools.add_bundle(StaticToolBundle::new(
            [&found, &deferred]
                .into_iter()
                .map(|name| {
                    Arc::new(ScriptedTool::new(
                        ToolSpec {
                            name: name.clone(),
                            description: "fixture t3-code MCP tool".into(),
                            input_schema: parameters.clone(),
                        },
                        ScriptedToolOutcome::Success(ToolOutput::text("called")),
                    )) as Arc<dyn rho_sdk::tool::Tool>
                })
                .collect(),
        ));
        let provider = ScriptedProvider::new(
            ModelIdentity::new("test", "test", "test"),
            [
                tool_call(
                    "nested",
                    CODEMODE_TOOL_NAME,
                    json!({"script": r#"list_tools()
hit = search_tools("unwatch_pull_request")[0]
describe_tool(hit["name"])
result = call_tool(hit["name"], {"threadId": "thread"})"#}),
                ),
                tool_call("before_search", &found, json!({"threadId": "thread"})),
                tool_call(
                    "search",
                    TOOL_SEARCH_NAME,
                    // Both tools match t3-code; only the returned hit is loaded.
                    json!({"query": "t3-code unwatch_pull_request", "limit": 1}),
                ),
                ScriptedTurn::completed(ModelResponse::Assistant(
                    [("after_search", &found), ("unsearched", &deferred)]
                        .into_iter()
                        .map(|(id, name)| {
                            ContentBlock::ToolCall(ToolCall {
                                id: id.into(),
                                name: name.clone(),
                                arguments: json!({"threadId": "thread"}),
                            })
                        })
                        .collect(),
                )),
                text_turn(),
            ],
        );
        let runtime = runtime_for(
            &config,
            &tools,
            &provider,
            Workspace::new(std::env::current_dir().unwrap()).unwrap(),
        );
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        session.complete("stop watching").await.unwrap();

        assert_eq!(
            session
                .history()
                .into_iter()
                .filter_map(|message| match message {
                    Message::ToolResult(result) => Some((result.id, result.ok)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                ("nested".into(), true),
                ("before_search".into(), false),
                ("search".into(), true),
                ("after_search".into(), true),
                ("unsearched".into(), false),
            ],
            "mode: {mode:?}"
        );
        let mut expected = tools.tool_named(&found).unwrap().spec();
        if let Some(description) = tools.tool_visibility().describe(&expected) {
            expected.description = description;
        }
        for (index, request) in provider.recorded_requests().iter().enumerate() {
            let mcp_specs: Vec<_> = request
                .tools
                .iter()
                .filter(|spec| spec.name.starts_with("mcp__"))
                .cloned()
                .collect();
            assert_eq!(
                mcp_specs,
                if index >= 3 {
                    vec![expected.clone()]
                } else {
                    vec![]
                },
                "mode: {mode:?}, request: {index}"
            );
        }
    }
}
