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

/// Codemode is opt-in; these tests exercise exposure with it enabled.
fn codemode_config() -> Config {
    Config {
        codemode: true,
        ..Config::default()
    }
}

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

fn runtime_for(
    config: &Config,
    tools: &AppToolSet,
    provider: &ScriptedProvider,
    workspace: Workspace,
    mode: PermissionMode,
) -> rho_sdk::Rho {
    let shared: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    build_runtime(RuntimeBuildOptions {
        provider: shared,
        tools: tools.tools(),
        tool_visibility: tools.tool_visibility(),
        workspace,
        workspace_policy: AppPolicy::for_mode(mode, Default::default()),
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

// Covers: the provider tool list follows exposure per request — MCP tools
// default to codemode-only (absent), hidden tools stay absent, natives stay
// direct, and a tool_search promotion is advertised on the next request of
// the same run.
// Owner: app runtime builder + exposure (provider boundary).
#[tokio::test]
async fn provider_tool_list_follows_exposure_and_promotion() {
    let config = codemode_config();
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
    let runtime = runtime_for(
        &config,
        &tools,
        &provider,
        Workspace::new(std::env::current_dir().unwrap()).unwrap(),
        PermissionMode::Auto,
    );
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

fn advertised(provider: &ScriptedProvider, request: usize, names: &[&str]) -> Vec<(String, bool)> {
    let tools = &provider.recorded_requests()[request].tools;
    names
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                tools.iter().any(|spec| spec.name == *name),
            )
        })
        .collect()
}

// Covers: `/codemode on` write-locks the provider tool list (Write/Process
// natives, including a promoted deferred one, are not advertised; read tools
// and codemode are), `/codemode off` restores them and drops codemode, and
// toggling keeps promotion state.
// Owner: app tool set write lock + SDK visibility (provider boundary).
#[tokio::test]
async fn codemode_toggle_write_locks_provider_tool_list() {
    let watched = [
        "read_file",
        "list_dir",
        CODEMODE_TOOL_NAME,
        TOOL_SEARCH_NAME,
        "write",
        "edit",
        "bash",
        "deferred_writer",
    ];
    let config = codemode_config();
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.add_bundle(FixtureBundle(vec![Arc::new(WriteFixture)]));
    tools.set_exposure_policy(
        ExposurePolicy::new().override_exact("deferred_writer", ToolExposure::Deferred),
    );
    let promoted = {
        let provider = ScriptedProvider::new(
            ModelIdentity::new("test", "test", "test"),
            [
                tool_call("p-1", TOOL_SEARCH_NAME, json!({"query": "deferred_writer"})),
                text_turn(),
            ],
        );
        let runtime = runtime_for(
            &config,
            &tools,
            &provider,
            Workspace::new(std::env::current_dir().unwrap()).unwrap(),
            PermissionMode::Auto,
        );
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        session.complete("find it").await.unwrap();
        provider
    };
    // Promotion happened but the write lock keeps it off the next request.
    assert_eq!(
        advertised(&promoted, 1, &["deferred_writer"]),
        vec![("deferred_writer".to_owned(), false)]
    );

    let cases = [
        (
            /*codemode*/ true,
            vec![true, true, true, true, false, false, false, false],
        ),
        (
            /*codemode*/ false,
            vec![true, true, false, true, true, true, true, true],
        ),
        (
            /*codemode*/ true,
            vec![true, true, true, true, false, false, false, false],
        ),
    ];
    for (codemode, expected) in cases {
        tools.set_codemode_registered(codemode);
        let provider =
            ScriptedProvider::new(ModelIdentity::new("test", "test", "test"), [text_turn()]);
        let runtime = runtime_for(
            &config,
            &tools,
            &provider,
            Workspace::new(std::env::current_dir().unwrap()).unwrap(),
            PermissionMode::Auto,
        );
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        session.complete("hi").await.unwrap();
        assert_eq!(
            advertised(&provider, 0, &watched),
            watched
                .iter()
                .zip(expected)
                .map(|(name, visible)| ((*name).to_owned(), visible))
                .collect::<Vec<_>>(),
            "codemode={codemode}"
        );
    }
}

// Covers: under the write lock a direct model `write` does not execute, while
// the same mutation through `codemode` succeeds under the session permission
// mode (no codemode-specific permission level).
// Owner: app write lock routing + nested ToolHost authorization.
#[tokio::test(flavor = "multi_thread")]
async fn write_lock_routes_mutation_through_codemode() {
    let root = tempfile::tempdir().unwrap();
    let config = codemode_config();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    assert!(tools.codemode_registered());
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
        PermissionMode::Bypass,
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    session.complete("write files").await.unwrap();

    assert_eq!(
        (
            root.path().join("direct.txt").exists(),
            std::fs::read_to_string(root.path().join("nested.txt")).ok(),
        ),
        (false, Some("nested".to_owned()))
    );
}

/// Deferred tool that declares `Write` authority without touching disk.
struct WriteFixture;

impl Tool for WriteFixture {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "deferred_writer".into(),
            description: "deferred_writer".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn security(&self) -> rho_sdk::tool::ToolSecurity {
        rho_sdk::tool::ToolSecurity::built_in([rho_sdk::CapabilityKind::Write])
    }

    fn call<'a>(
        &'a self,
        _invocation: rho_sdk::tool::ToolInvocation,
        _context: rho_sdk::tool::ToolContext,
    ) -> rho_sdk::tool::ToolFuture<'a> {
        Box::pin(async { Ok(ToolOutput::text("wrote")) })
    }
}

fn text_turn() -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        "done".into(),
    )]))
}
