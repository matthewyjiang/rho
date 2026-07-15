use std::sync::{Arc, Mutex};

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
    builder = builder.tool_shared(Arc::new(super::super::sdk_shell::SdkShellTool::bash(
        12_000,
    )));
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
    assert_eq!(security_for("web_search").origin(), ToolOrigin::BuiltIn);
    assert_eq!(
        security_for("web_search").capabilities(),
        [CapabilityKind::Network]
    );
    assert_eq!(security_for("rho").origin(), ToolOrigin::BuiltIn);
    assert!(security_for("rho").capabilities().is_empty());
}
