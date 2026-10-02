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
    // The workspace shell tool is `powershell` on Windows.
    let shell = crate::config::default_inline_shell();
    let watched = [
        CODEMODE_TOOL_NAME,
        TOOL_SEARCH_NAME,
        "read_file",
        "write",
        shell.as_str(),
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

// Covers: Pi's on-mode hint at the provider boundary. Declared natives end
// with a line naming the script call and its result shape (bash: its
// structured fields; read_file: the {content} fallback); codemode itself and
// every tool in `only` mode are sent unchanged.
// Owner: ExposureController ToolVisibility::describe.
#[cfg(unix)]
#[tokio::test]
async fn on_mode_descriptions_name_script_result_shape() {
    use crate::config::CodemodeMode::{On, Only};
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let mut observed = Vec::new();
    for mode in [On, Only] {
        tools.set_codemode_mode(mode);
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
        let declared = provider.recorded_requests()[0].tools.clone();
        let last_line = |name: &str| {
            declared.iter().find(|spec| spec.name == name).map(|spec| {
                spec.description
                    .lines()
                    .last()
                    .unwrap_or_default()
                    .to_owned()
            })
        };
        observed.push((
            mode,
            last_line("bash"),
            last_line("read_file"),
            last_line(CODEMODE_TOOL_NAME).is_some_and(|line| line.starts_with("Codemode:")),
        ));
    }
    assert_eq!(
        observed,
        vec![
            (
                On,
                Some(
                    "Codemode: `call_tool(\"bash\", args)` returns \
`{ stdout, stderr, exit_code, truncated, wall_time_ms }`."
                        .to_owned()
                ),
                Some(
                    "Codemode: `call_tool(\"read_file\", args)` returns `{ content }`.".to_owned()
                ),
                false,
            ),
            (Only, None, None, false),
        ]
    );
}

// Covers: real workspace tools hand codemode scripts typed results to loop and
// branch on: glob paths, grep files/lines, list_dir entry kinds, and a process
// started then polled to exit with its exit_code and next_cursor.
// Owner: built-in tool structured content, end to end through codemode.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn script_reads_structured_workspace_and_process_results() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/a.rs"), "fn a() {}\n// TODO one\n").unwrap();
    std::fs::write(root.path().join("src/b.rs"), "// TODO two\n// TODO three\n").unwrap();
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let script = r#"
paths = call_tool("glob", {"pattern": "*.rs"})["paths"]
hits = call_tool("grep", {"pattern": "TODO"})
todo = {f["path"]: [l["line"] for l in f["lines"]] for f in hits["files"]}
kinds = {e["name"]: e["kind"] for e in call_tool("list_dir", {"path": "."})["entries"]}
started = call_tool("process", {"action": "start", "command": "printf hi; exit 4"})
# A poll returns as soon as output arrives, so follow next_cursor until exit.
cursor = 0
out = ""
for _ in range(20):
    polled = call_tool("process", {"action": "poll", "process_id": started["process_id"], "cursor": cursor, "wait_seconds": 5})
    out += polled["stdout"]
    cursor = polled["next_cursor"]
    if polled["state"] not in ("running", "starting"):
        break
result = {
    "paths": sorted(paths),
    "todo": todo,
    "total": hits["total_matches"],
    "stopped": hits["stopped"],
    "kinds": kinds,
    "process": [polled["state"], polled["exit_code"], out, cursor > 0],
}
"#;
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            tool_call("script", CODEMODE_TOOL_NAME, json!({ "script": script })),
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

    session.complete("survey").await.unwrap();

    let output = session
        .history()
        .iter()
        .find_map(|message| match message {
            rho_sdk::model::Message::ToolResult(result) if result.id == "script" => {
                Some((result.ok, result.content.clone()))
            }
            _ => None,
        })
        .unwrap();
    assert!(output.0, "{}", output.1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.1).unwrap(),
        json!({
            "paths": ["src/a.rs", "src/b.rs"],
            "todo": {"src/a.rs": [2], "src/b.rs": [1, 2]},
            "total": 3,
            "stopped": [],
            "kinds": {"src": "dir"},
            "process": ["exited", 4, "hi", true],
        })
    );
}

/// Nested tool that asks the host one question and returns the answer.
struct AskTool;

impl Tool for AskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask".into(),
            description: "ask the user one question".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(
        &'a self,
        _invocation: rho_sdk::tool::ToolInvocation,
        context: rho_sdk::tool::ToolContext,
    ) -> rho_sdk::tool::ToolFuture<'a> {
        Box::pin(async move {
            let question = rho_sdk::HostQuestion::new(
                "answer",
                "choose",
                vec![rho_sdk::HostChoice::new("yes", "yes")],
                rho_sdk::SelectionMode::One,
            )
            .unwrap();
            let request =
                rho_sdk::HostInputRequest::questionnaire("nested", vec![question]).unwrap();
            let response = context.request_host_input(request).await.map_err(|error| {
                rho_sdk::tool::ToolError::new(
                    rho_sdk::tool::ToolErrorKind::Execution,
                    error.to_string(),
                )
            })?;
            Ok(ToolOutput::text(response.answers()["answer"][0].clone()))
        })
    }
}

// Covers: a nested tool's host question surfaces on the session run and its
// answer reaches the script. The script must not block the task that relays
// the parent call's host-input channel, or the question never arrives. The
// live-wiring tests drive ToolHost directly and cannot see this.
// Owner: codemode CodeModeTool script thread + ToolHostBridge host-input relay.
#[tokio::test(flavor = "multi_thread")]
async fn nested_host_question_reaches_the_session_and_answers_the_script() {
    let config = Config::default();
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    tools.add_bundle(FixtureBundle(vec![Arc::new(AskTool)]));
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            tool_call(
                "script",
                CODEMODE_TOOL_NAME,
                json!({"script": r#"result = call_tool("ask", {})"#}),
            ),
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
    let mut run = session
        .start(rho_sdk::UserInput::text("ask"))
        .await
        .unwrap();

    let request = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let rho_sdk::RunEvent::ToolHostInputRequested { call_id, request } =
                run.next_event().await.expect("run ended before asking")
            {
                assert_eq!(call_id.to_string(), "script");
                break request;
            }
        }
    })
    .await
    .expect("nested question never reached the session");
    run.respond(
        request.id().clone(),
        rho_sdk::HostInputResponse::new().answer("answer", ["yes"]),
    )
    .await
    .unwrap();
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");

    let requests = provider.recorded_requests();
    let result = requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            rho_sdk::model::Message::ToolResult(result) if result.id == "script" => Some(result),
            _ => None,
        })
        .expect("codemode result reached the model");
    assert!(result.ok);
    assert!(format!("{:?}", result.content).contains("yes"));
}

// Covers: nested status lines stay live while the script runs and never
// deadlock. Twelve calls emit 24+ updates, far past the parent progress
// channel's capacity (4, `sdk_config::parallel_tool_limit`); the script must
// finish, and the last update before the codemode call finishes must show
// every call done rather than an early `running` line kept when later updates
// were dropped.
// Owner: codemode ToolHostBridge progress reporting.
#[tokio::test(flavor = "multi_thread")]
async fn nested_status_reaches_the_final_state_before_the_call_finishes() {
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
                "script",
                CODEMODE_TOOL_NAME,
                json!({"script": r#"
for _ in range(12):
    call_tool("list_dir", {"path": "."})
result = "done"
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
    let mut run = session
        .start(rho_sdk::UserInput::text("loop"))
        .await
        .unwrap();

    let mut last_status = None;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(event) = run.next_event().await {
            match event {
                rho_sdk::RunEvent::ToolUpdated { call_id, progress }
                    if call_id.to_string() == "script" =>
                {
                    last_status = Some(progress.text().to_owned());
                }
                rho_sdk::RunEvent::ToolFinished { call_id, .. }
                    if call_id.to_string() == "script" =>
                {
                    break
                }
                _ => {}
            }
        }
    })
    .await
    .expect("codemode call never finished");

    let last_status = last_status.expect("no nested status reached the session");
    assert_eq!(last_status, ["list_dir: done"; 12].join("\n"));
}
