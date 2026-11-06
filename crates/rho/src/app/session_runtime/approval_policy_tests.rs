use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolOutput},
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, ApprovalSession,
    CapabilityRequest, CapabilitySource, PathScope, PolicyDecision, UserInput, WorkspacePolicy,
};
use tokio::sync::mpsc;

use super::{ensure_auto_classifier_model, ApprovalPolicy, UnattendedApprovals};
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    app::{
        policy::AppPolicy,
        session_assembly::ApprovalInputs,
        session_runtime::{
            build::with_plan_exit,
            test_support::{built_on, test_build},
            SessionEvent, SessionRuntime, SessionStorage, Turn, TurnControl, TurnIo, TurnOutcome,
        },
    },
    config::{Config, InternalAgentModelConfig},
    permission::{PermissionMode, SessionWriteLog},
    permission_classifier::ClassifierVerdict,
    permission_classifier_handler::{ClassifierApprovalHandler, CONSECUTIVE_DENY_ESCALATION},
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

// Covers: Auto must fail before a session builds when no classifier model is configured.
// Owner: session build permission gating.
#[test]
fn auto_requires_configured_permission_classifier_model() {
    let mut config = Config {
        permission_mode: PermissionMode::Auto,
        ..Config::default()
    };

    let error = ensure_auto_classifier_model(&config).unwrap_err();
    assert_eq!(
        error.to_string(),
        "permission mode auto requires a configured permission-classifier model (set via /config or config.toml [internal_agents.permission-classifier])"
    );

    config.set_internal_agent_model_config(
        PERMISSION_CLASSIFIER_AGENT_ID,
        InternalAgentModelConfig::new("openai".into(), "gpt-5.5".into(), "api-key".into()),
    );
    ensure_auto_classifier_model(&config).unwrap();
}

// Covers: a classifier allow or human escalation on a parent's Auto
// template must record into this session's write log, not the template's,
// so a later untracked workspace write skips classification.
// Owner: session runtime approval wiring.
#[tokio::test]
async fn classifier_template_records_allows_on_the_session_write_log() {
    let root = tempfile::tempdir().unwrap();
    let created = root.path().join("new.rs");
    std::fs::write(&created, "fn main() {}\n").unwrap();
    let git_status = std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git should be available for permission tests");
    assert!(git_status.success(), "git init failed");

    let mut config = Config {
        permission_mode: PermissionMode::Auto,
        ..Config::default()
    };
    config.set_internal_agent_model_config(
        PERMISSION_CLASSIFIER_AGENT_ID,
        InternalAgentModelConfig::new("openai".into(), "gpt-5.5".into(), "api-key".into()),
    );
    let request = CapabilityRequest::write_path(
        created,
        PathScope::PrimaryWorkspace,
        CapabilitySource::built_in_tool("write"),
    );
    let require_approval = PolicyDecision::RequireApproval {
        reason: String::new(),
    };

    // Whether the classifier denies until the human it escalates to allows.
    for escalate in [false, true] {
        let classify_calls = Arc::new(AtomicUsize::new(0));
        let template_writes = SessionWriteLog::default();
        let session_writes = SessionWriteLog::default();
        let template = Arc::new(
            ClassifierApprovalHandler::for_tests(
                {
                    let classify_calls = Arc::clone(&classify_calls);
                    Arc::new(move |_input| {
                        classify_calls.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async move {
                            if escalate {
                                ClassifierVerdict::Deny {
                                    reason: "blocked".into(),
                                }
                            } else {
                                ClassifierVerdict::Allow
                            }
                        })
                    })
                },
                escalate.then(|| Arc::new(AllowEverything) as Arc<dyn ApprovalHandler>),
            )
            .with_session_writes(template_writes.clone()),
        );
        let handler =
            UnattendedApprovals::ClassifierTemplate(template).classifier(ApprovalInputs {
                config: config.clone(),
                workspace_root: root.path().to_path_buf(),
                usage_recording: Default::default(),
                session_writes: session_writes.clone(),
            });
        let session_policy = AppPolicy::for_mode(PermissionMode::Auto, session_writes);
        let template_policy = AppPolicy::for_mode(PermissionMode::Auto, template_writes);
        assert_eq!(session_policy.evaluate(&request), require_approval);

        if escalate {
            for _ in 0..CONSECUTIVE_DENY_ESCALATION {
                assert!(matches!(
                    handler
                        .request(ApprovalRequest::new(request.clone(), "approval required"))
                        .await,
                    ApprovalDecision::Deny { .. }
                ));
            }
        }
        let decision = handler
            .request(ApprovalRequest::new(request.clone(), "approval required"))
            .await;
        let expected_calls = if escalate {
            CONSECUTIVE_DENY_ESCALATION as usize
        } else {
            1
        };
        assert_eq!(
            (
                decision,
                classify_calls.load(Ordering::SeqCst),
                session_policy.evaluate(&request),
                template_policy.evaluate(&request),
            ),
            (
                ApprovalDecision::AllowOnce,
                expected_calls,
                PolicyDecision::Allow,
                require_approval.clone(),
            ),
            "escalate: {escalate}"
        );
    }
}
