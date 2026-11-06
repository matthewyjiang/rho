use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolOutput},
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, ApprovalSession,
    CapabilityRequest, CapabilitySource, PathScope, UserInput,
};
use tokio::sync::mpsc;

use super::{ApprovalPolicy, UnattendedApprovals};
use crate::{
    app::session_runtime::{
        build::with_plan_exit,
        test_support::{built_on, test_build},
        SessionEvent, SessionRuntime, SessionStorage, Turn, TurnControl, TurnIo, TurnOutcome,
    },
    config::Config,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::{
        plan_exit,
        sdk_registry::{AppToolSet, StaticToolBundle},
    },
};

const PROBE: &str = "write_probe";

/// Writes a workspace file through the real authorization path and records
/// whether it was allowed.
struct WriteProbe {
    allowed: Arc<Mutex<Option<bool>>>,
}

impl Tool for WriteProbe {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: PROBE.into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let path = context
            .workspace_root()
            .expect("the session has a workspace")
            .join("file.txt");
        Box::pin(async move {
            let authorized = context
                .authorize(CapabilityRequest::write_path(
                    path,
                    PathScope::PrimaryWorkspace,
                    CapabilitySource::built_in_tool(PROBE),
                ))
                .await;
            *self.allowed.lock().unwrap() = Some(authorized.is_ok());
            authorized.map_err(|error| ToolError::policy_denied(&error))?;
            Ok(ToolOutput::text("wrote"))
        })
    }
}

/// What a workflow's approval session does when it allows everything.
struct AllowEverything;

impl ApprovalHandler for AllowEverything {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async { ApprovalDecision::AllowOnce })
    }
}

/// What one session came to: whether its host was asked about a
/// supervised write, whether the write was allowed, and whether plan mode
/// then offered `exit_plan_mode`.
#[derive(Debug, PartialEq)]
struct Outcome {
    host_asked: bool,
    allowed: Option<bool>,
    plan_exit: bool,
}

// Covers: in a mode that asks before writing, only a host-answered session
// shows the request to its host. An unattended session answers it itself:
// with nothing inherited it denies the write, and an inherited workflow
// approval session decides instead. Neither waits on the host, and neither
// offers a plan exit only a host could approve.
// Owner: session runtime approval wiring, from the session's first build
// through permission mode rebuilds (scripted provider, no credentials).
#[tokio::test]
async fn approvals_reach_the_host_only_when_it_answers_them() {
    let cases = [
        (
            "host",
            ApprovalPolicy::Host,
            Outcome {
                host_asked: true,
                allowed: Some(true),
                plan_exit: true,
            },
        ),
        (
            "unattended alone",
            ApprovalPolicy::Unattended(UnattendedApprovals::Alone),
            Outcome {
                host_asked: false,
                allowed: Some(false),
                plan_exit: false,
            },
        ),
        (
            "unattended with an inherited session",
            ApprovalPolicy::Unattended(UnattendedApprovals::Inherited(ApprovalSession::new(
                AllowEverything,
            ))),
            Outcome {
                host_asked: false,
                allowed: Some(true),
                plan_exit: false,
            },
        ),
    ];
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    };
    for (label, approvals, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let stored = StoredSession::create_in_root(root.path(), root.path()).unwrap();
        let id = rho_sdk::SessionId::from_string(stored.id()).unwrap();
        let provider = Arc::new(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "model"),
            vec![
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall::new("write".into(), PROBE.into(), serde_json::json!({})),
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ));
        let allowed = Arc::new(Mutex::new(None));
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(StaticToolBundle::new(vec![Arc::new(WriteProbe {
            allowed: Arc::clone(&allowed),
        })]));
        // The session is made with the policy from the start, and its first
        // build adds the tools the policy offers, as `build_session` does.
        let mut build = test_build(&config, root.path());
        build.host.approvals = approvals;
        let mut built = built_on(&config, id, provider).await;
        built.tools = with_plan_exit(tools, &build, &build.launch.agent, &config);
        let mut runtime =
            SessionRuntime::for_tests_with(build, built, SessionStorage::saved(stored), &config);
        // Entering supervised rebuilds the SDK runtime with the tools and the
        // policy's approval wiring, as a host's mode change does.
        runtime
            .set_permission_mode(PermissionMode::Supervised, &config)
            .await
            .unwrap();

        let prompt = runtime.begin_prompt(&config).await.unwrap();
        let (events, mut shown) = mpsc::unbounded_channel();
        let (controls, control) = mpsc::unbounded_channel();
        // A host that allows what it is asked, so a request that wrongly
        // reaches it shows as an allowed write rather than a hung turn.
        let host = async move {
            let mut asked = false;
            while let Some(event) = shown.recv().await {
                if let SessionEvent::ApprovalRequested { id, .. } = event {
                    asked = true;
                    let _ = controls.send(TurnControl::RespondApproval {
                        id,
                        decision: ApprovalDecision::AllowOnce,
                    });
                }
            }
            asked
        };
        let input = UserInput::text("write it");
        let turn = Turn {
            display: input.clone(),
            model: input,
            calls: Vec::new(),
        };
        // A scripted turn ends in milliseconds. The bound only turns a turn
        // left waiting on an approval nobody answers into a failure rather
        // than a hung test.
        let (ended, host_asked) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(
                runtime.run_turn(&prompt, turn, TurnIo { events, control }, &config),
                host,
            )
        })
        .await
        .unwrap_or_else(|_| panic!("{label}: the turn hung on an unanswered approval"));
        assert!(
            matches!(ended, Ok(TurnOutcome::Completed(_))),
            "{label}: {ended:?}"
        );
        drop(prompt);
        let allowed = *allowed.lock().unwrap();
        runtime
            .set_permission_mode(PermissionMode::Plan, &config)
            .await
            .unwrap();
        let plan_exit = runtime.tools().contains(plan_exit::TOOL_NAME);

        assert_eq!(
            Outcome {
                host_asked,
                allowed,
                plan_exit,
            },
            expected,
            "{label}"
        );
        runtime.shutdown().await;
    }
}
