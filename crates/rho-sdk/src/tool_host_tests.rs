use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    approval_channel,
    hooks::{
        HookDecision, HookEventKind, HookGateFuture, HookHostLabels, HookObserver, PreToolUseGate,
        PreToolUseRequest,
    },
    model::ToolSpec,
    tool::{
        Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
        ToolProgress,
    },
    ApprovalAuditDecision, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    ApprovalSession, AuthorizationDenialKind, CapabilityRequest, CapabilitySource, Error,
    HostChoice, HostInputRequest, HostInputResponse, HostQuestion, PathScope, PolicyDecision,
    SelectionMode, ToolHost, ToolHostCall, ToolHostEvent, WorkspacePolicy,
};

#[derive(Clone)]
struct OrderedPolicy {
    order: Arc<Mutex<Vec<&'static str>>>,
}

impl WorkspacePolicy for OrderedPolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        self.order.lock().unwrap().push("policy");
        PolicyDecision::RequireApproval {
            reason: "test approval".into(),
        }
    }
}

struct OrderedGate {
    order: Arc<Mutex<Vec<&'static str>>>,
    decision: HookDecision,
    requests: Arc<Mutex<Vec<crate::hooks::HookEnvelope>>>,
}

impl PreToolUseGate for OrderedGate {
    fn evaluate(&self, request: PreToolUseRequest) -> HookGateFuture<'_> {
        self.order.lock().unwrap().push("hook");
        self.requests
            .lock()
            .unwrap()
            .push(request.envelope().clone());
        Box::pin(std::future::ready(self.decision.clone()))
    }
}

struct OrderedApproval {
    order: Arc<Mutex<Vec<&'static str>>>,
    decision: ApprovalDecision,
}

impl ApprovalHandler for OrderedApproval {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        self.order.lock().unwrap().push("approval");
        Box::pin(std::future::ready(self.decision.clone()))
    }
}

#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<crate::hooks::HookEnvelope>>,
}

impl HookObserver for RecordingObserver {
    fn observe(&self, envelope: crate::hooks::HookEnvelope) {
        self.events.lock().unwrap().push(envelope);
    }
}

struct AuthorizingTool {
    order: Arc<Mutex<Vec<&'static str>>>,
}

impl Tool for AuthorizingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "host_exec".into(),
            description: "authorize one host operation".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let path = invocation
                .arguments()
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("/work/input");
            context.authorize(capability(path)).await.map_err(|error| {
                if matches!(error.kind(), AuthorizationDenialKind::Cancelled) {
                    ToolError::cancelled()
                } else {
                    ToolError::policy_denied(&error)
                }
            })?;
            self.order.lock().unwrap().push("execution");
            Ok(ToolOutput::text("done"))
        })
    }
}

fn capability(path: &str) -> CapabilityRequest {
    CapabilityRequest::read_path(
        path,
        PathScope::PrimaryWorkspace,
        CapabilitySource::host_tool("host_exec"),
    )
}

fn call() -> ToolHostCall {
    ToolHostCall::new("host_exec", json!({"input": "value"}))
}

fn call_path(path: &str) -> ToolHostCall {
    ToolHostCall::new("host_exec", json!({"path": path}))
}

// Covers: a host tool must not bypass or reorder any authorization stage when no provider exists.
// Owner: SDK ToolHost authorization orchestration.
#[tokio::test]
async fn provider_free_call_preserves_authorization_order_and_hook_pairing() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::new(RecordingObserver::default());
    let gate_requests = Arc::new(Mutex::new(Vec::new()));
    let host = ToolHost::builder()
        .tool(AuthorizingTool {
            order: Arc::clone(&order),
        })
        .workspace_policy(OrderedPolicy {
            order: Arc::clone(&order),
        })
        .pre_tool_gate_shared(Arc::new(OrderedGate {
            order: Arc::clone(&order),
            decision: HookDecision::Continue,
            requests: Arc::clone(&gate_requests),
        }))
        .approval_handler(OrderedApproval {
            order: Arc::clone(&order),
            decision: ApprovalDecision::AllowOnce,
        })
        .hook_observer_shared(observer.clone())
        .hook_host_labels(
            HookHostLabels::new()
                .label("run", "host-run")
                .label("node", "build"),
        )
        .build()
        .unwrap();

    let output = host.invoke(call()).await.unwrap();

    assert_eq!(output.content(), "done");
    assert_eq!(
        *order.lock().unwrap(),
        vec!["policy", "hook", "approval", "execution"]
    );
    let gate_requests = gate_requests.lock().unwrap();
    let events = observer.events.lock().unwrap();
    assert_eq!(
        (gate_requests[0].event(), events[0].event()),
        (HookEventKind::BeforeToolUse, HookEventKind::AfterToolUse)
    );
    assert_eq!(gate_requests[0].host_labels().get("run"), Some("host-run"));
    assert_eq!(events[0].host_labels().get("node"), Some("build"));
}

// Covers: a deny-only hook must stop a host call before approval and execution.
// Owner: SDK ToolHost authorization orchestration.
#[tokio::test]
async fn hook_denial_stops_before_approval() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let host = ToolHost::builder()
        .tool(AuthorizingTool {
            order: Arc::clone(&order),
        })
        .workspace_policy(OrderedPolicy {
            order: Arc::clone(&order),
        })
        .pre_tool_gate_shared(Arc::new(OrderedGate {
            order: Arc::clone(&order),
            decision: HookDecision::deny("blocked by test hook"),
            requests: Arc::default(),
        }))
        .approval_handler(OrderedApproval {
            order: Arc::clone(&order),
            decision: ApprovalDecision::AllowOnce,
        })
        .build()
        .unwrap();

    let error = host.invoke(call()).await.unwrap_err();

    assert!(matches!(
        error,
        Error::Tool(ref error) if error.kind() == ToolErrorKind::PolicyDenied
    ));
    assert_eq!(*order.lock().unwrap(), vec!["policy", "hook"]);
    assert_eq!(
        host.approval_audit()
            .iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![ApprovalAuditDecision::DeniedByHook]
    );
}

struct CountingApproval {
    count: Arc<Mutex<usize>>,
}

impl ApprovalHandler for CountingApproval {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        *self.count.lock().unwrap() += 1;
        Box::pin(std::future::ready(ApprovalDecision::AllowForSession))
    }
}

// Covers: AllowForSession must apply to exact later calls on one ToolHost session.
// Owner: SDK ToolHost approval memory.
#[tokio::test]
async fn exact_approval_is_remembered_for_the_tool_host_session() {
    let count = Arc::new(Mutex::new(0));
    let approvals = ApprovalSession::new(CountingApproval {
        count: Arc::clone(&count),
    });
    let host = ToolHost::builder()
        .tool(AuthorizingTool {
            order: Arc::default(),
        })
        .workspace_policy(OrderedPolicy {
            order: Arc::default(),
        })
        .approval_session(approvals.clone())
        .build()
        .unwrap();

    host.invoke(call()).await.unwrap();
    let later_host = ToolHost::builder()
        .tool(AuthorizingTool {
            order: Arc::default(),
        })
        .workspace_policy(OrderedPolicy {
            order: Arc::default(),
        })
        .approval_session(approvals)
        .build()
        .unwrap();
    later_host.invoke(call()).await.unwrap();
    later_host.invoke(call_path("/work/other")).await.unwrap();

    assert_eq!(*count.lock().unwrap(), 2);
    assert_eq!(
        later_host
            .approval_audit()
            .iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![
            ApprovalAuditDecision::AllowedForSession,
            ApprovalAuditDecision::AllowedByRememberedApproval,
            ApprovalAuditDecision::AllowedForSession,
        ]
    );
}

// Covers: host denial must stop a provider-free tool before execution.
// Owner: SDK ToolHost authorization orchestration.
#[tokio::test]
async fn host_denial_stops_before_execution() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let host = ToolHost::builder()
        .tool(AuthorizingTool {
            order: Arc::clone(&order),
        })
        .workspace_policy(OrderedPolicy {
            order: Arc::clone(&order),
        })
        .approval_handler(OrderedApproval {
            order: Arc::clone(&order),
            decision: ApprovalDecision::Deny {
                reason: "denied by test host".into(),
            },
        })
        .build()
        .unwrap();

    let error = host.invoke(call()).await.unwrap_err();

    assert!(matches!(
        error,
        Error::Tool(ref error) if error.kind() == ToolErrorKind::PolicyDenied
    ));
    assert_eq!(*order.lock().unwrap(), vec!["policy", "approval"]);
    assert_eq!(
        host.approval_audit()
            .iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![ApprovalAuditDecision::DeniedByHost]
    );
}

// Covers: cancelling while approval waits must stop before execution and record cancellation.
// Owner: SDK ToolHost cancellation and approval orchestration.
#[tokio::test]
async fn cancellation_interrupts_a_pending_approval() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let (approval_handler, mut approvals) =
        approval_channel(NonZeroUsize::new(1).expect("one is nonzero"));
    let host = ToolHost::builder()
        .tool(AuthorizingTool {
            order: Arc::clone(&order),
        })
        .workspace_policy(OrderedPolicy {
            order: Arc::clone(&order),
        })
        .approval_handler(approval_handler)
        .build()
        .unwrap();
    let mut run = host.start(call()).unwrap();

    let pending = approvals.recv().await.expect("approval request");
    assert_eq!(pending.request().tool_call_id(), Some(run.call_id()));
    run.cancel();
    let error = run.outcome().await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Tool(ref error) if error.kind() == ToolErrorKind::Cancelled
        ),
        "unexpected cancellation error: {error:?}"
    );
    assert_eq!(*order.lock().unwrap(), vec!["policy"]);
    assert_eq!(
        host.approval_audit()
            .iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![ApprovalAuditDecision::Cancelled]
    );
}

struct InteractiveTool;

impl Tool for InteractiveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "interactive".into(),
            description: "report progress and ask one question".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            context
                .progress()
                .send(ToolProgress::message("ready"))
                .await;
            let question = HostQuestion::new(
                "choice",
                "Continue?",
                vec![HostChoice::new("yes", "Yes")],
                SelectionMode::One,
            )
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let request = HostInputRequest::questionnaire("Confirm", vec![question])
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let response = context
                .request_host_input(request)
                .await
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            Ok(ToolOutput::text(response.answers()["choice"].join(",")))
        })
    }
}

// Covers: provider-free tools must retain progress and typed host-input interaction.
// Owner: SDK ToolHost event boundary.
#[tokio::test]
async fn call_emits_progress_and_accepts_host_input() {
    let host = ToolHost::builder().tool(InteractiveTool).build().unwrap();
    let mut run = host
        .start(ToolHostCall::new("interactive", json!({})))
        .unwrap();

    let ToolHostEvent::Progress(progress) = run.next_event().await.unwrap() else {
        panic!("first event must be progress")
    };
    assert_eq!(progress.text(), "ready");
    let ToolHostEvent::HostInputRequested(mut pending) = run.next_event().await.unwrap() else {
        panic!("second event must request host input")
    };
    pending
        .respond(HostInputResponse::new().answer("choice", ["yes"]))
        .unwrap();

    assert_eq!(run.outcome().await.unwrap().content(), "yes");
}
