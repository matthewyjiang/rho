use std::sync::Arc;

use pretty_assertions::assert_eq;

use crate::{
    hooks::{
        gate::{HookDecision, HookGateFuture, PreToolUseGate, PreToolUseRequest},
        payload::{HookPayload, SessionStartedPayload},
        testing::RecordingObserver,
        HookEventKind,
    },
    SessionId,
};

use super::*;

struct DenyGate;

impl PreToolUseGate for DenyGate {
    fn evaluate(&self, _request: PreToolUseRequest) -> HookGateFuture<'_> {
        Box::pin(std::future::ready(HookDecision::deny("no")))
    }
}

fn runtime_with(observer: Option<Arc<dyn HookObserver>>) -> HookWiring {
    HookWiring::new(
        observer,
        /* gate */ None,
        HookPayloadBounds::default(),
        HookDelegation::default(),
    )
}

#[test]
fn a_delegated_runtime_reports_its_parent_identity() {
    let observer = Arc::new(RecordingObserver::default());
    let parent_session = SessionId::from_string("parent-session").unwrap();
    let hooks = HookWiring::new(
        Some(observer.clone()),
        /* gate */ None,
        HookPayloadBounds::default(),
        HookDelegation::new(parent_session.clone()),
    );
    let child = SessionId::from_string("child-session").unwrap();

    hooks.observe(Some(&child), None, None, |_| {
        HookPayload::SessionStarted(SessionStartedPayload {
            model: "scripted/test".into(),
        })
    });

    let seen = observer.envelopes();
    assert_eq!(
        seen[0].identity(),
        &HookIdentity {
            session_id: Some(child),
            parent_session_id: Some(parent_session),
            run_id: None,
        }
    );
}

// Covers: a dispatcher is enabled by a gate alone, and disabled without hooks.
// Owner: SDK hook dispatch
#[tokio::test]
async fn a_dispatcher_is_enabled_only_when_hooks_are_wired() {
    let gate_only = HookWiring::new(
        /* observer */ None,
        Some(Arc::new(DenyGate)),
        HookPayloadBounds::default(),
        HookDelegation::default(),
    );

    assert!(HookDispatcher::new(gate_only, None).is_enabled());
    assert!(!HookDispatcher::new(HookWiring::default(), None).is_enabled());
}

#[tokio::test]
async fn a_runtime_without_a_gate_lets_every_request_continue() {
    let hooks = runtime_with(None);
    let request = crate::hooks::testing::before_tool_use_request(
        "bash",
        crate::hooks::HookPolicyOutcome::Allow,
    );

    let decision = hooks.evaluate_pre_tool_use(request).await;

    assert_eq!(decision, HookDecision::Continue);
}

#[test]
fn the_host_dispatcher_reports_both_session_boundaries() {
    let observer = Arc::new(RecordingObserver::default());
    let dispatcher =
        HookDispatcher::new(runtime_with(Some(observer.clone())), Some("/work".into()));
    let session = SessionId::from_string("session-1").unwrap();

    assert!(dispatcher.is_enabled());
    dispatcher.session_completed(&session, 4);
    dispatcher.session_failed(
        &session,
        HookSessionFailureKind::Provider,
        "provider failed: overloaded",
    );

    assert_eq!(
        observer.events(),
        vec![
            HookEventKind::SessionCompleted,
            HookEventKind::SessionFailed
        ]
    );
    let seen = observer.envelopes();
    assert_eq!(
        serde_json::to_value(seen[0].payload()).unwrap(),
        serde_json::json!({ "runs": 4 })
    );
    assert_eq!(
        serde_json::to_value(seen[1].payload()).unwrap(),
        serde_json::json!({
            "failure": { "kind": "provider", "message": "provider failed: overloaded" },
        })
    );
}
