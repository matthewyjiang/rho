//! End-to-end hook behavior over a real run.
//!
//! These cover the contract the issue fixes in place: where the gate sits
//! relative to policy and approval, what a denial looks like to the model, and
//! which events a run emits in which order.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    hooks::{
        HookDecision, HookEnvelope, HookEventKind, HookGateFuture, HookObserveFuture, HookObserver,
        HookPolicyOutcome, PreToolUseGate, PreToolUseRequest,
    },
    model::{ContentBlock, ModelIdentity, ModelResponse, ModelUsage, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput},
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CapabilityRequest,
    CapabilitySource, PathScope, PolicyDecision, Rho, SessionOptions, Workspace, WorkspacePolicy,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

/// Tool that asks for one read capability, so it flows through the whole
/// policy-then-hook-then-approval path.
struct ReadingTool;

impl Tool for ReadingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "reads a file".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            context
                .authorize(CapabilityRequest::read_path(
                    "/work/notes.txt",
                    PathScope::PrimaryWorkspace,
                    CapabilitySource::built_in_tool("read_file"),
                ))
                .await
                .map_err(|error| crate::tool::ToolError::policy_denied(&error))?;
            Ok(ToolOutput::text("file contents"))
        })
    }
}

struct FixedPolicy(PolicyDecision);

impl WorkspacePolicy for FixedPolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        self.0.clone()
    }
}

#[derive(Default)]
struct CountingApprovals {
    prompts: Mutex<usize>,
}

impl ApprovalHandler for CountingApprovals {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            *self.prompts.lock().unwrap() += 1;
            ApprovalDecision::AllowOnce
        })
    }
}

struct ScriptedGate {
    decision: HookDecision,
    seen: Mutex<Vec<HookPolicyOutcome>>,
    envelopes: Mutex<Vec<HookEnvelope>>,
}

impl ScriptedGate {
    fn new(decision: HookDecision) -> Arc<Self> {
        Arc::new(Self {
            decision,
            seen: Mutex::new(Vec::new()),
            envelopes: Mutex::new(Vec::new()),
        })
    }

    fn consulted(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl PreToolUseGate for ScriptedGate {
    fn evaluate(&self, request: PreToolUseRequest) -> HookGateFuture<'_> {
        self.seen.lock().unwrap().push(request.policy());
        self.envelopes
            .lock()
            .unwrap()
            .push(request.envelope().clone());
        let decision = self.decision.clone();
        Box::pin(async move { decision })
    }
}

#[derive(Default)]
struct RecordingObserver {
    seen: Mutex<Vec<HookEnvelope>>,
}

impl RecordingObserver {
    fn events(&self) -> Vec<HookEventKind> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(HookEnvelope::event)
            .collect()
    }
}

impl HookObserver for RecordingObserver {
    fn observe(&self, envelope: HookEnvelope) -> HookObserveFuture<'_> {
        Box::pin(async move {
            self.seen.lock().unwrap().push(envelope);
        })
    }
}

fn tool_then_text() -> ScriptedProvider {
    ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    )
}

struct Harness {
    runtime: Rho,
    gate: Arc<ScriptedGate>,
    approvals: Arc<CountingApprovals>,
    observer: Arc<RecordingObserver>,
}

fn harness(policy: PolicyDecision, decision: HookDecision) -> Harness {
    let gate = ScriptedGate::new(decision);
    let approvals = Arc::new(CountingApprovals::default());
    let observer = Arc::new(RecordingObserver::default());
    let workspace = Workspace::new(std::env::temp_dir()).unwrap();
    let runtime = Rho::builder()
        .provider(tool_then_text())
        .tool(ReadingTool)
        .workspace(workspace)
        .workspace_policy(FixedPolicy(policy))
        .approval_handler_shared(approvals.clone())
        .pre_tool_gate_shared(gate.clone())
        .hook_observer_shared(observer.clone())
        .build()
        .unwrap();
    Harness {
        runtime,
        gate,
        approvals,
        observer,
    }
}

/// Content the model sees for the denied tool call.
async fn tool_result_content(harness: &Harness) -> String {
    let session = harness
        .runtime
        .session(SessionOptions::default())
        .await
        .unwrap();
    session.complete("go").await.unwrap();
    session
        .history()
        .into_iter()
        .find_map(|message| match message {
            crate::model::Message::ToolResult(result) => Some(result.content),
            _ => None,
        })
        .expect("the run recorded a tool result")
}

#[tokio::test]
async fn allow_plus_continue_executes_without_prompting() {
    let harness = harness(PolicyDecision::Allow, HookDecision::Continue);

    let content = tool_result_content(&harness).await;

    assert_eq!(content, "file contents");
    assert_eq!(harness.gate.consulted(), 1);
    assert_eq!(*harness.approvals.prompts.lock().unwrap(), 0);
}

#[tokio::test]
async fn allow_plus_deny_stops_the_call() {
    let harness = harness(
        PolicyDecision::Allow,
        HookDecision::deny("hook `user:no-reads` denied the read"),
    );

    let content = tool_result_content(&harness).await;

    assert!(
        content.contains("hook `user:no-reads` denied the read"),
        "the model must be able to read why the call failed: {content}"
    );
    assert_eq!(*harness.approvals.prompts.lock().unwrap(), 0);
}

#[tokio::test]
async fn require_approval_plus_continue_still_prompts_the_host() {
    let harness = harness(
        PolicyDecision::RequireApproval {
            reason: "ask".into(),
        },
        HookDecision::Continue,
    );

    let content = tool_result_content(&harness).await;

    assert_eq!(content, "file contents");
    assert_eq!(*harness.approvals.prompts.lock().unwrap(), 1);
    assert_eq!(
        harness.gate.seen.lock().unwrap().as_slice(),
        [HookPolicyOutcome::RequireApproval]
    );
}

#[tokio::test]
async fn require_approval_plus_deny_stops_before_the_prompt() {
    let harness = harness(
        PolicyDecision::RequireApproval {
            reason: "ask".into(),
        },
        HookDecision::deny("denied by policy hook"),
    );

    let content = tool_result_content(&harness).await;

    assert!(content.contains("denied by policy hook"));
    assert_eq!(
        *harness.approvals.prompts.lock().unwrap(),
        0,
        "a hook denial must arrive before the host is asked"
    );
}

#[tokio::test]
async fn a_policy_denial_never_reaches_the_gate() {
    let harness = harness(
        PolicyDecision::Deny {
            reason: "capability is outside the configured policy".into(),
        },
        HookDecision::Continue,
    );

    let content = tool_result_content(&harness).await;

    assert!(content.contains("outside the configured policy"));
    assert_eq!(
        harness.gate.consulted(),
        0,
        "hooks cannot widen a decision, so they are not asked about one already denied"
    );
}

#[tokio::test]
async fn a_hook_denial_is_recorded_as_its_own_audit_kind() {
    let harness = harness(PolicyDecision::Allow, HookDecision::deny("no"));

    tool_result_content(&harness).await;

    let audit = harness.runtime.diagnostics();
    assert!(audit
        .approval_audit()
        .iter()
        .any(|record| record.decision() == crate::ApprovalAuditDecision::DeniedByHook));
}

#[tokio::test]
async fn a_successful_run_emits_session_then_tool_then_run_events() {
    let harness = harness(PolicyDecision::Allow, HookDecision::Continue);

    tool_result_content(&harness).await;

    // `before_tool_use` is a question, not a notification: it goes to the gate
    // exactly once and is not repeated to the observational sink.
    assert_eq!(
        harness.observer.events(),
        vec![
            HookEventKind::SessionStarted,
            HookEventKind::AfterToolUse,
            HookEventKind::RunCompleted,
        ]
    );
    assert_eq!(harness.gate.consulted(), 1);
}

#[tokio::test]
async fn the_gate_receives_a_before_tool_use_envelope_with_the_command_it_must_judge() {
    let harness = harness(PolicyDecision::Allow, HookDecision::Continue);

    tool_result_content(&harness).await;

    let seen = harness.gate.envelopes.lock().unwrap();
    let envelope = seen.first().expect("the gate was consulted once");
    assert_eq!(envelope.event(), HookEventKind::BeforeToolUse);
    let payload = serde_json::to_value(envelope.payload()).unwrap();
    assert_eq!(payload["tool"]["name"], json!("read_file"));
    assert_eq!(payload["tool"]["call_id"], json!("call-1"));
    assert_eq!(payload["capability_kind"], json!("read"));
    assert_eq!(payload["capability"]["operation"], json!("read_path"));
    assert_eq!(payload["policy"], json!("allow"));
}

#[tokio::test]
async fn after_tool_use_reports_the_call_that_a_hook_denied() {
    let harness = harness(PolicyDecision::Allow, HookDecision::deny("no"));

    tool_result_content(&harness).await;

    let seen = harness.observer.seen.lock().unwrap();
    let after = seen
        .iter()
        .find(|envelope| envelope.event() == HookEventKind::AfterToolUse)
        .expect("the denied call still resolved");
    let payload = serde_json::to_value(after.payload()).unwrap();
    assert_eq!(payload["tool"]["name"], json!("read_file"));
    assert_eq!(payload["tool"]["call_id"], json!("call-1"));
    assert_eq!(payload["status"], json!("failed"));
}

#[tokio::test]
async fn every_tool_event_carries_the_session_and_run_it_belongs_to() {
    let harness = harness(PolicyDecision::Allow, HookDecision::Continue);
    let session = harness
        .runtime
        .session(SessionOptions::default())
        .await
        .unwrap();

    session.complete("go").await.unwrap();

    let seen = harness.observer.seen.lock().unwrap();
    let run_ids: Vec<_> = seen
        .iter()
        .filter(|envelope| envelope.event() != HookEventKind::SessionStarted)
        .map(|envelope| envelope.identity().run_id.clone())
        .collect();
    assert!(run_ids.iter().all(Option::is_some));
    assert_eq!(
        run_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1,
        "one run must report one run ID across its events"
    );
    assert!(seen
        .iter()
        .all(|envelope| envelope.identity().session_id.as_ref() == Some(session.id())));
}

#[tokio::test]
async fn a_failed_run_reports_run_failed_with_a_typed_kind() {
    let observer = Arc::new(RecordingObserver::default());
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming_failed(
            Vec::new(),
            crate::ProviderError::new(
                crate::ProviderErrorKind::Unavailable,
                "overloaded",
                crate::Retryability::Permanent,
            ),
        )],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .hook_observer_shared(observer.clone())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    session.complete("go").await.unwrap_err();

    assert_eq!(
        observer.events(),
        vec![HookEventKind::SessionStarted, HookEventKind::RunFailed]
    );
    let seen = observer.seen.lock().unwrap();
    assert_eq!(
        serde_json::to_value(seen[1].payload()).unwrap()["failure"]["kind"],
        json!("provider")
    );
}

#[tokio::test]
async fn run_events_fire_per_run_while_session_events_fire_once() {
    let observer = Arc::new(RecordingObserver::default());
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "one".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "two".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .hook_observer_shared(observer.clone())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    session.complete("first").await.unwrap();
    session.complete("second").await.unwrap();
    runtime.hooks().session_completed(session.id(), 2).await;

    assert_eq!(
        observer.events(),
        vec![
            HookEventKind::SessionStarted,
            HookEventKind::RunCompleted,
            HookEventKind::RunCompleted,
            HookEventKind::SessionCompleted,
        ]
    );
}

#[tokio::test]
async fn a_runtime_without_hooks_runs_unchanged() {
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("plain".into()),
            ]))],
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = session.complete("go").await.unwrap();

    assert_eq!(outcome.text(), "plain");
    assert!(!runtime.hooks().is_enabled());
    assert_eq!(outcome.usage(), &ModelUsage::default());
}
