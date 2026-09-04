use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ToolErrorKind, ToolOrigin},
    ApprovalAuditDecision, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CapabilityKind, CapabilityOperation, CapabilitySource, ProcessEnvironment, Rho, RunEvent,
    ScopedWorkspacePolicy, SessionOptions, ToolCompletion, UserInput, Workspace,
};
use serde_json::json;

use super::*;

fn capabilities(names: &[&str]) -> AgentCapabilities {
    AgentCapabilities::new(
        names
            .iter()
            .map(|name| ToolCapability::parse((*name).to_string()))
            .collect(),
    )
}

struct RegistryWorkflowService;

impl super::super::workflow::WorkflowToolService for RegistryWorkflowService {
    fn execute<'a>(
        &'a self,
        _request: super::super::workflow::WorkflowToolRequest,
        _context: &'a rho_sdk::tool::ToolContext,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        super::super::workflow::WorkflowToolResult,
                        rho_sdk::tool::ToolError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Ok(super::super::workflow::WorkflowToolResult::Validate {
                valid: true,
                diagnostics: Vec::new(),
            })
        })
    }
}

// Covers: hook matcher names must not drift from the application tool registry.
// Owner: application tool registry.
#[test]
fn canonical_tool_names_match_the_unfiltered_registry() {
    fn normalized(names: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut names = names
            .into_iter()
            .map(|name| match name.as_str() {
                "bash" | "powershell" => "shell".to_owned(),
                _ => name,
            })
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        names
    }

    let root = tempfile::tempdir().unwrap();
    let mut model_names = Vec::new();
    for edit_tool in [
        crate::config::EditTool::Pinned(rho_tools::EditFormat::Hashline),
        crate::config::EditTool::Pinned(rho_tools::EditFormat::ApplyPatch),
        crate::config::EditTool::Pinned(rho_tools::EditFormat::StrReplace),
    ] {
        let config = Config {
            edit_tool,
            ..Config::default()
        };
        let options = ToolSetOptions::default()
            .advisor(AdvisorSessionStore::new())
            .delegation(DelegationConfig::new(
                root.path().to_owned(),
                root.path().join("config.toml"),
                BackgroundSubagents::Enabled,
                /*catalog*/ None,
            ))
            .workflow(Arc::new(RegistryWorkflowService));
        let mut tools = AppToolSet::new(&config, RuntimeDiagnostics::new(&config), options);
        // Advisor mode is off by default; the registry still owns the name.
        tools.set_advisor_registered(true);
        let names = tools.unfiltered_names().collect::<Vec<_>>();
        let selected = config.resolved_edit_tool().tool_name();
        for name in ["edit", "apply_patch", "str_replace"] {
            assert_eq!(
                names.iter().any(|candidate| candidate == name),
                name == selected,
                "edit_tool={edit_tool:?} name={name}"
            );
        }
        model_names.extend(names);
    }

    assert!(!model_names.iter().any(|name| name == "workflow_command"));
    assert!(!model_names.iter().any(|name| name == "message_parent"));
    let registry_names = model_names.into_iter().chain(
        super::super::HOST_ONLY_TOOL_NAMES
            .iter()
            .chain(super::super::DELEGATED_OPT_IN_TOOL_NAMES.iter())
            .map(|name| (*name).to_owned()),
    );

    assert_eq!(
        normalized(registry_names),
        normalized(
            super::super::canonical_tool_names()
                .iter()
                .map(|name| (*name).to_owned())
        )
    );
}

struct RecordingBundle {
    tools: Vec<Arc<dyn Tool>>,
    shutdown: Arc<AtomicBool>,
}

impl ToolBundle for RecordingBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move { self.shutdown.store(true, Ordering::SeqCst) })
    }
}

#[tokio::test]
async fn shuts_down_feature_bundles_through_the_generic_lifecycle() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut tool_set = AppToolSet::disabled();
    tool_set.add_bundle(RecordingBundle {
        tools: Vec::new(),
        shutdown: shutdown.clone(),
    });

    tool_set.shutdown().await;

    assert!(shutdown.load(Ordering::SeqCst));
}

#[derive(Debug)]
struct RecordingApprovals {
    requests: Mutex<Vec<ApprovalRequest>>,
}

impl ApprovalHandler for RecordingApprovals {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(request);
            ApprovalDecision::Deny {
                reason: "host rejected process execution".into(),
            }
        })
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_shell_input_reaches_approval_as_structured_process_facts() {
    let root = tempfile::tempdir().unwrap();
    let command = "touch should-not-exist; printf '%s' '$TOKEN; && | $(touch quoted-not-exist)'";
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "shell-1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": command, "timeout_seconds": 9}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "denial handled".into(),
            )])),
        ],
    );
    let approvals = Arc::new(RecordingApprovals {
        requests: Mutex::new(Vec::new()),
    });
    let config = Config {
        max_output_bytes: 777,
        rtk: true,
        ..Config::default()
    };
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let bash = tool_set
        .tools()
        .iter()
        .find(|tool| tool.spec().name == "bash")
        .unwrap()
        .clone();
    let mut builder = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(root.path()).unwrap())
        .workspace_policy(
            ScopedWorkspacePolicy::new()
                .allow_processes()
                .require_process_approval(),
        )
        .approval_handler_shared(approvals.clone());
    builder = builder.tool_shared(bash);
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("run it")).await.unwrap();
    let mut failure = None;
    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ToolFinished {
                result: ToolCompletion::Failure(tool_failure),
                ..
            } => failure = Some(tool_failure),
            RunEvent::Completed { outcome } => {
                assert_eq!(outcome.text(), "denial handled");
                break;
            }
            _ => {}
        }
    }

    let failure = failure.unwrap();
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    assert!(failure.message().contains("process capability denied"));
    assert!(!root.path().join("should-not-exist").exists());

    let requests = approvals
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.capability().source(),
        &CapabilitySource::built_in_tool("bash")
    );
    let CapabilityOperation::ExecuteProcess(execution) = request.capability().operation() else {
        panic!("expected process approval");
    };
    assert_eq!(
        execution.working_directory(),
        root.path().canonicalize().unwrap()
    );
    assert_eq!(
        execution.invocation().executable_path(),
        std::path::Path::new("bash")
    );
    assert_eq!(execution.invocation().arguments(), ["-lc"]);
    assert_eq!(execution.invocation().shell_command(), Some(command));
    assert_eq!(
        execution.environment(),
        &ProcessEnvironment::inherit_except(rho_providers::credential_env_vars())
    );
    assert_eq!(execution.output_limits().max_output_bytes(), 777);
    assert_eq!(execution.output_limits().timeout().unwrap().as_secs(), 9);
    assert!(!format!("{request:?}").contains("$TOKEN"));
    drop(requests);

    let diagnostics = runtime.diagnostics();
    let bash = diagnostics
        .tools()
        .iter()
        .find(|tool| tool.name() == "bash")
        .unwrap();
    assert_eq!(bash.origin(), ToolOrigin::BuiltIn);
    assert_eq!(bash.capabilities(), [CapabilityKind::Process]);
    assert_eq!(
        diagnostics
            .approval_audit()
            .iter()
            .map(|record| (record.capability(), record.decision()))
            .collect::<Vec<_>>(),
        [(CapabilityKind::Process, ApprovalAuditDecision::DeniedByHost)]
    );
    assert!(!format!("{diagnostics:?}").contains("$TOKEN"));
}

#[cfg(unix)]
#[tokio::test]
async fn sdk_shell_tools_stream_live_output_as_progress_events() {
    let root = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "shell-1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "printf 'live-marker\\n'; sleep 0.3"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let mut builder = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(root.path()).unwrap())
        .workspace_policy(ScopedWorkspacePolicy::new().allow_processes());
    builder = builder.tool_shared(rho_tools::shell_tool(
        rho_tools::ShellToolOptions::new().max_output_bytes(12_000),
    ));
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("run it")).await.unwrap();
    let mut progress_messages = Vec::new();
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolUpdated { progress, .. } = event {
            progress_messages.push(progress.text().to_string());
        }
    }
    run.outcome().await.unwrap();

    assert!(
        progress_messages
            .iter()
            .any(|message| message.contains("live-marker")),
        "expected live output in progress events: {progress_messages:?}"
    );
}

#[tokio::test]
async fn sdk_skill_tool_loads_discovered_skill_outside_workspace_root() {
    let root = tempfile::tempdir().unwrap();
    let workspace_root = root.path().join("project/workspace");
    let skill_dir = root.path().join("project/.agents/skills/ancestor-skill");
    std::fs::create_dir_all(root.path().join("project/.git")).unwrap();
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: ancestor-skill\ndescription: ancestor skill\n---\nancestor body\n",
    )
    .unwrap();

    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "skill-1".into(),
                    name: "skill".into(),
                    arguments: json!({"name": "ancestor-skill"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let skill = tool_set
        .tools()
        .iter()
        .find(|tool| tool.spec().name == "skill")
        .unwrap()
        .clone();
    let mut builder = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(&workspace_root).unwrap())
        .workspace_policy(ScopedWorkspacePolicy::new().allow_skills());
    builder = builder.tool_shared(skill);
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("load it")).await.unwrap();
    let mut output = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished {
            result: ToolCompletion::Success(completion),
            ..
        } = event
        {
            output = Some(completion.content().to_string());
        }
    }
    run.outcome().await.unwrap();

    let output = output.unwrap();
    assert!(output.contains("Loaded skill: ancestor-skill"));
    assert!(output.lines().any(|line| {
        line.starts_with("Source: ")
            && line.ends_with("/project/.agents/skills/ancestor-skill/SKILL.md")
    }));
    assert!(output.lines().any(|line| {
        line.starts_with("References are relative to ")
            && line.ends_with("/project/.agents/skills/ancestor-skill.")
    }));
    assert!(output.ends_with("ancestor body\n"));
}

#[tokio::test]
async fn sdk_skill_tool_rejects_model_invocation_of_user_only_skill() {
    let root = tempfile::tempdir().unwrap();
    let skill_dir = root.path().join(".agents/skills/manual-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: manual-skill\ndescription: manual skill\ndisable-model-invocation: true\n---\nmanual body\n",
    )
    .unwrap();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "skill-1".into(),
                    name: "skill".into(),
                    arguments: json!({"name": "manual-skill"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let skill = tool_set
        .tools()
        .iter()
        .find(|tool| tool.spec().name == "skill")
        .unwrap()
        .clone();
    let runtime = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(root.path()).unwrap())
        .workspace_policy(ScopedWorkspacePolicy::new().allow_skills())
        .tool_shared(skill)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("load it")).await.unwrap();
    let mut failure = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished {
            result: ToolCompletion::Failure(tool_failure),
            ..
        } = event
        {
            failure = Some(tool_failure);
        }
    }
    run.outcome().await.unwrap();

    let failure = failure.expect("model-requested skill should fail");
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    assert_eq!(
        failure.message(),
        "skill 'manual-skill' requires direct user invocation"
    );
}

#[tokio::test]
async fn sdk_skill_tool_loads_embedded_agent_creator_without_workspace() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let skill = tool_set
        .tools()
        .iter()
        .find(|tool| tool.spec().name == "skill")
        .unwrap()
        .clone();
    let mut builder = Rho::builder()
        .provider(provider)
        .workspace_policy(ScopedWorkspacePolicy::new().allow_skills());
    builder = builder.tool_shared(skill);
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start_with_tool_call(
            UserInput::text("/skill:rho-agent-creator"),
            ToolCall {
                id: "skill-creator-1".into(),
                name: "skill".into(),
                arguments: json!({"name": "rho-agent-creator"}),
            },
        )
        .await
        .unwrap();
    let mut output = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished {
            result: ToolCompletion::Success(completion),
            ..
        } = event
        {
            output = Some(completion.content().to_string());
        }
    }
    run.outcome().await.unwrap();

    assert!(output
        .as_deref()
        .is_some_and(|content| content.contains("questionnaire")));
}

#[test]
fn security_declarations_distinguish_network_builtins_from_host_tools() {
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&["web_search", "rho"])),
    );
    let security = |name: &str| {
        tool_set
            .tools()
            .iter()
            .find(|tool| tool.spec().name == name)
            .expect("selected tool")
            .security()
    };

    let web_search = security("web_search");
    assert_eq!(web_search.origin(), ToolOrigin::BuiltIn);
    assert_eq!(web_search.capabilities(), [CapabilityKind::Network]);
    let rho = security("rho");
    assert_eq!(rho.origin(), ToolOrigin::BuiltIn);
    assert!(rho.capabilities().is_empty());
}

// Covers: /advisor must add and remove the advisor tool mid-session without
// disturbing the rest of the tool set or dropping the configured model.
// Owner: application tool registry.
#[test]
fn advisor_registration_toggles_without_rebuilding_the_tool_set() {
    let config = Config::default();
    let store = AdvisorSessionStore::new();
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&["advisor", "read_file"])).advisor(store),
    );
    let without_advisor = tools.unfiltered_names().collect::<Vec<_>>();

    assert!(!tools.advisor_registered());
    assert!(!without_advisor.iter().any(|name| name == "advisor"));

    for (requested, changed, expected) in [
        (true, true, true),
        (true, false, true),
        (false, true, false),
        (false, false, false),
    ] {
        assert_eq!(
            tools.set_advisor_registered(requested),
            changed,
            "requested={requested}"
        );
        assert_eq!(
            tools.advisor_registered(),
            expected,
            "requested={requested}"
        );
        assert_eq!(tools.contains("advisor"), expected, "requested={requested}");
    }

    assert_eq!(
        tools.unfiltered_names().collect::<Vec<_>>(),
        without_advisor
    );
    // The store outlives the registration, so turning the mode back on keeps
    // the model the user already chose.
    assert!(tools.advisor().is_some());
}

// Covers: /config edit-tool selection must swap the single advertised edit
// surface without rebuilding the rest of the tool set.
// Owner: application tool registry.
#[test]
fn edit_tool_selection_swaps_the_advertised_edit_surface() {
    let config = Config {
        edit_tool: crate::config::EditTool::Pinned(rho_tools::EditFormat::Hashline),
        ..Config::default()
    };
    let mut tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&["edit", "read_file"])),
    );
    let before = tools.unfiltered_names().collect::<Vec<_>>();
    assert_eq!(tools.edit_tool(), Some(rho_tools::EditFormat::Hashline));
    assert!(tools.contains("edit"));
    assert!(!tools.contains("str_replace"));

    assert_eq!(
        tools.set_edit_tool(rho_tools::EditFormat::Hashline, config.max_output_bytes),
        None
    );
    assert_eq!(
        tools.set_edit_tool(rho_tools::EditFormat::StrReplace, config.max_output_bytes),
        Some(rho_tools::EditFormat::Hashline)
    );
    assert_eq!(tools.edit_tool(), Some(rho_tools::EditFormat::StrReplace));
    assert!(!tools.contains("edit"));
    assert!(tools.contains("str_replace"));

    let after = tools.unfiltered_names().collect::<Vec<_>>();
    assert_eq!(before.len(), after.len());
    assert!(after.iter().any(|name| name == "read_file"));
    assert_eq!(
        after.iter().filter(|name| *name == "read_file").count(),
        1,
        "edit-tool switch must keep one read_file"
    );
    assert_eq!(tools.file_view_style(), rho_tools::FileViewStyle::Numbered);

    let mut without_edit = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&["read_file"])),
    );
    assert_eq!(
        without_edit.set_edit_tool(rho_tools::EditFormat::ApplyPatch, config.max_output_bytes),
        None
    );
}

// Covers: Auto preference advertises the provider-preferred format at construction.
// Owner: application tool registry.
#[test]
fn auto_edit_tool_constructs_the_preferred_provider_format() {
    let config = Config {
        provider: "anthropic".into(),
        edit_tool: crate::config::EditTool::Auto,
        ..Config::default()
    };
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&["edit", "read_file"])),
    );
    assert_eq!(tools.edit_tool(), Some(rho_tools::EditFormat::StrReplace));
    assert!(tools.contains("str_replace"));
    assert!(!tools.contains("edit"));
    assert!(!tools.contains("apply_patch"));
}
