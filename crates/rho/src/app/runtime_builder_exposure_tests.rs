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

fn runtime_for(
    config: &Config,
    tools: &AppToolSet,
    provider: &ScriptedProvider,
    workspace: Workspace,
) -> rho_sdk::Rho {
    let shared: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    build_runtime(RuntimeBuildOptions {
        provider: shared,
        tools: tools.tools(),
        tool_visibility: tools.tool_visibility(),
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
    let runtime = runtime_for(
        &config,
        &tools,
        &provider,
        Workspace::new(std::env::current_dir().unwrap()).unwrap(),
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

// Covers: Pi `codemode.mode` at the provider boundary. `on` declares natives
// next to codemode; `only` hides policy-direct natives while codemode and
// tool_search stay declared, and an already-promoted deferred tool stays
// declared (its exposure is deferred, not direct). Switching back restores.
// MCP stays codemode-only in both modes.
// Owner: app exposure + SDK per-request visibility.
#[tokio::test]
async fn codemode_mode_selects_provider_tool_list() {
    use crate::config::CodemodeMode::{On, Only};
    let watched = [
        CODEMODE_TOOL_NAME,
        TOOL_SEARCH_NAME,
        "read_file",
        "write",
        "bash",
        "mcp__docs__lookup",
        "rare_report",
    ];
    let config = Config::default();
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.add_bundle(FixtureBundle(vec![
        tool("mcp__docs__lookup", "look up documentation pages"),
        tool("rare_report", "generate the quarterly widget report"),
    ]));
    tools.set_exposure_policy(
        ExposurePolicy::new().override_exact("rare_report", ToolExposure::Deferred),
    );
    assert!(tools.exposure().promote("rare_report"));

    let cases = [
        (On, [true, true, true, true, true, false, true]),
        (Only, [true, true, false, false, false, false, true]),
        (On, [true, true, true, true, true, false, true]),
    ];
    let mut observed = Vec::new();
    for (mode, _) in &cases {
        tools.set_codemode_mode(*mode);
        let provider =
            ScriptedProvider::new(ModelIdentity::new("test", "test", "test"), [text_turn()]);
        let runtime = runtime_for(
            &config,
            &tools,
            &provider,
            Workspace::new(std::env::current_dir().unwrap()).unwrap(),
        );
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        session.complete("hi").await.unwrap();
        let declared = &provider.recorded_requests()[0].tools;
        observed.push((
            *mode,
            watched.map(|name| declared.iter().any(|spec| spec.name == name)),
        ));
    }
    assert_eq!(observed, cases.to_vec());
}

// Covers: in `only` a direct model call to a hidden native does not execute,
// while the same native runs through `codemode` under the session permission
// mode (`only` is presentation, not a permission level).
// Owner: app codemode only routing + nested ToolHost authorization.
#[tokio::test(flavor = "multi_thread")]
async fn only_mode_routes_natives_through_codemode() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.set_codemode_mode(crate::config::CodemodeMode::Only);
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
            std::fs::read_to_string(root.path().join("nested.txt")).ok(),
        ),
        (false, Some("nested".to_owned()))
    );
}

// Covers: the real `bash` tool's structured content reaches a codemode script,
// so it branches on a nonzero `exit_code` and keeps going (Pi semantics), while
// the same command called directly still fails the tool call.
// Owner: shell structured outcome + codemode bridge, end to end.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn script_branches_on_bash_exit_code() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            tool_call(
                "direct",
                "bash",
                json!({"command": "printf oops >&2; exit 3"}),
            ),
            tool_call(
                "script",
                CODEMODE_TOOL_NAME,
                json!({"script": r#"
run = call_tool("bash", {"command": "printf oops >&2; exit 3"})
result = {"code": run["exit_code"], "stderr": run["stderr"], "truncated": run["truncated"]} if run["exit_code"] != 0 else "unreachable"
"#}),
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

    session.complete("run it").await.unwrap();

    let results = session
        .history()
        .iter()
        .filter_map(|message| match message {
            rho_sdk::model::Message::ToolResult(result) => Some((result.id.clone(), result.ok)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results,
        vec![("direct".to_owned(), false), ("script".to_owned(), true)]
    );
    let script_output = session
        .history()
        .iter()
        .find_map(|message| match message {
            rho_sdk::model::Message::ToolResult(result) if result.id == "script" => {
                Some(serde_json::from_str::<serde_json::Value>(&result.content).unwrap())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        script_output,
        json!({"code": 3, "stderr": "oops", "truncated": false})
    );
}
