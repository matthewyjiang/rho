//! Provider-boundary proof that app tool exposure reaches model requests.

use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, Tool, ToolOutput},
    SessionOptions, SystemPrompt, Workspace,
};
use serde_json::json;

use super::{build_runtime, RuntimeBuildOptions};
use crate::{
    app::policy::AppPolicy,
    compaction::CompactionConfig,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    tools::{
        code_mode::{ExposurePolicy, ToolExposure, CODEMODE_TOOL_NAME, TOOL_SEARCH_NAME},
        sdk_registry::{AppToolSet, ToolBundle, ToolSetOptions},
    },
};

struct FixtureBundle(Vec<Arc<dyn Tool>>);

impl ToolBundle for FixtureBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.0
    }
}

fn tool(name: &str, description: &str) -> Arc<dyn Tool> {
    Arc::new(ScriptedTool::new(
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text(format!("{name} ran"))),
    ))
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

// Covers: the provider tool list follows exposure per request — MCP tools
// default to codemode-only (absent), hidden tools stay absent, natives stay
// direct, and a tool_search promotion is advertised on the next request of
// the same run.
// Owner: app runtime builder + exposure (provider boundary).
#[tokio::test]
async fn provider_tool_list_follows_exposure_and_promotion() {
    let config = Config::default();
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.add_bundle(FixtureBundle(vec![
        tool("mcp__docs__lookup", "look up documentation pages"),
        tool("rare_report", "generate the quarterly widget report"),
        tool("secret_admin", "administer secrets"),
    ]));
    tools.set_exposure_policy(
        ExposurePolicy::new()
            .override_exact("rare_report", ToolExposure::Deferred)
            .override_exact("secret_admin", ToolExposure::Hidden),
    );
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            tool_call("call-1", TOOL_SEARCH_NAME, json!({"query": "widget"})),
            tool_call("call-2", "rare_report", json!({})),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let shared: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let runtime = build_runtime(RuntimeBuildOptions {
        provider: shared,
        tools: tools.tools(),
        tool_visibility: tools.tool_visibility(),
        workspace: Workspace::new(std::env::current_dir().unwrap()).unwrap(),
        workspace_policy: AppPolicy::for_mode(PermissionMode::Auto, Default::default()),
        approval_session: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        service_tier: None,
        compaction: CompactionConfig::from(&config),
        context_window: None,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        hooks: None,
        diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
        recall: None,
    })
    .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = session.complete("make the report").await.unwrap();

    assert_eq!(outcome.text(), "done");
    let advertised = |index: usize, name: &str| {
        provider.recorded_requests()[index]
            .tools
            .iter()
            .any(|spec| spec.name == name)
    };
    let watched = [
        "read_file",
        CODEMODE_TOOL_NAME,
        TOOL_SEARCH_NAME,
        "mcp__docs__lookup",
        "rare_report",
        "secret_admin",
    ];
    let snapshot = |index: usize| {
        watched
            .iter()
            .map(|name| (*name, advertised(index, name)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        (snapshot(0), snapshot(1)),
        (
            vec![
                ("read_file", true),
                (CODEMODE_TOOL_NAME, true),
                (TOOL_SEARCH_NAME, true),
                ("mcp__docs__lookup", false),
                ("rare_report", false),
                ("secret_admin", false),
            ],
            vec![
                ("read_file", true),
                (CODEMODE_TOOL_NAME, true),
                (TOOL_SEARCH_NAME, true),
                ("mcp__docs__lookup", false),
                ("rare_report", true),
                ("secret_admin", false),
            ],
        )
    );
    // The promoted call actually executed in the same run.
    assert!(session.history().iter().any(|message| matches!(
        message,
        rho_sdk::model::Message::ToolResult(result) if result.ok && result.content == "rare_report ran"
    )));
}
