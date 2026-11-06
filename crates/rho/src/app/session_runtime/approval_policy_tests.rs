use std::sync::{Arc, Mutex};

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
        test_support::built_on, SessionEvent, SessionRuntime, SessionStorage, Turn, TurnControl,
        TurnIo, TurnOutcome,
    },
    config::Config,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::sdk_registry::{AppToolSet, StaticToolBundle},
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

/// What one supervised write came to: whether the host was asked, and
/// whether the write was allowed.
#[derive(Debug, PartialEq)]
struct Write {
    host_asked: bool,
    allowed: Option<bool>,
}

// Covers: in a mode that asks before writing, only a host-answered session
// shows the request to its host. An unattended session answers it itself:
// with nothing inherited it denies the write, and an inherited workflow
// approval session decides instead. Neither waits on the host.
// Owner: session runtime approval wiring, through a permission mode rebuild
// (scripted provider, no credentials).
#[tokio::test]
async fn supervised_writes_reach_the_host_only_when_it_answers_approvals() {
    let cases = [
        (
            "host",
            ApprovalPolicy::Host,
            Write {
                host_asked: true,
                allowed: Some(true),
            },
        ),
        (
            "unattended alone",
            ApprovalPolicy::Unattended(UnattendedApprovals::Alone),
            Write {
                host_asked: false,
                allowed: Some(false),
            },
        ),
        (
            "unattended with an inherited session",
            ApprovalPolicy::Unattended(UnattendedApprovals::Inherited(ApprovalSession::new(
                AllowEverything,
            ))),
            Write {
                host_asked: false,
                allowed: Some(true),
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
        let mut built = built_on(&config, id, provider).await;
        built.tools = AppToolSet::disabled();
        built
            .tools
            .add_bundle(StaticToolBundle::new(vec![Arc::new(WriteProbe {
                allowed: Arc::clone(&allowed),
            })]));
        let mut runtime = SessionRuntime::for_tests(built, SessionStorage::saved(stored), &config);
        runtime.build.approvals = approvals;
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
        let (ended, host_asked) = tokio::join!(
            runtime.run_turn(&prompt, turn, TurnIo { events, control }, &config),
            host,
        );

        assert!(
            matches!(ended, Ok(TurnOutcome::Completed(_))),
            "{label}: {ended:?}"
        );
        let allowed = *allowed.lock().unwrap();
        assert_eq!(
            Write {
                host_asked,
                allowed
            },
            expected,
            "{label}"
        );
        drop(prompt);
        runtime.shutdown().await;
    }
}
