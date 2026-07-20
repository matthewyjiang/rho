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

fn delegation_options(names: &[&str], cwd: std::path::PathBuf) -> ToolSetOptions {
    ToolSetOptions::new(capabilities(names)).delegation(DelegationConfig::new(
        cwd,
        std::path::PathBuf::new(),
        BackgroundSubagents::Disabled,
    ))
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

#[test]
fn unselected_subagents_are_not_registered() {
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&[])),
    );
    let names: Vec<_> = tool_set.specs().into_iter().map(|spec| spec.name).collect();

    assert!(!names.contains(&"agent".to_string()));
    assert!(!names.contains(&"agents".to_string()));
    assert!(tool_set.subagents().is_none());
}

#[test]
fn constructs_only_selected_tools() {
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities(&["read_file"])),
    );

    assert_eq!(
        tool_set
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>(),
        vec!["read_file"]
    );
}

#[test]
fn delegation_tools_are_registered_independently() {
    for (allowed, expected) in [
        (["agent"].as_slice(), ["agent"].as_slice()),
        (["agents"].as_slice(), ["agents"].as_slice()),
        (
            ["agent", "agents"].as_slice(),
            ["agent", "agents"].as_slice(),
        ),
    ] {
        let config = Config::default();
        let root = tempfile::tempdir().unwrap();
        let tool_set = AppToolSet::new(
            &config,
            RuntimeDiagnostics::new(&config),
            delegation_options(allowed, root.path().to_path_buf()),
        );
        let names = tool_set
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .filter(|name| name == "agent" || name == "agents")
            .collect::<Vec<_>>();
        let expected = expected
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, expected);
        assert!(tool_set.subagents().is_some());
    }
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
    assert_eq!(execution.environment(), &ProcessEnvironment::InheritAll);
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
    builder = builder.tool_shared(rho_tools::shell_tool(12_000));
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

    assert_eq!(
        output.as_deref(),
        Some("---\nname: ancestor-skill\ndescription: ancestor skill\n---\nancestor body\n")
    );
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
