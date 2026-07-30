use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;

use crate::{
    hooks::{
        gate::{HookDecision, HookGateFuture, PreToolUseGate, PreToolUseRequest},
        payload::{HookPayload, SessionStartedPayload},
    },
    RunId, SessionId,
};

use super::*;

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

struct DenyGate;

impl PreToolUseGate for DenyGate {
    fn evaluate(&self, _request: PreToolUseRequest) -> HookGateFuture<'_> {
        Box::pin(std::future::ready(HookDecision::deny("no")))
    }
}

fn runtime_with(observer: Option<Arc<dyn HookObserver>>) -> HookRuntime {
    HookRuntime::new(
        observer,
        None,
        HookPayloadBounds::default(),
        HookDelegation::default(),
    )
}

#[tokio::test]
async fn a_runtime_without_an_observer_never_builds_a_payload() {
    let hooks = runtime_with(None);
    let built = Mutex::new(false);

    hooks
        .observe(HookEventKind::SessionStarted, None, None, None, |_| {
            *built.lock().unwrap() = true;
            HookPayload::SessionStarted(SessionStartedPayload {
                model: "scripted/test".into(),
            })
        })
        .await;

    assert!(
        !*built.lock().unwrap(),
        "payload construction must be skipped when nothing observes"
    );
    assert!(!hooks.observes());
}

#[tokio::test]
async fn an_observed_event_reaches_the_sink_with_its_identity() {
    let observer = Arc::new(RecordingObserver::default());
    let hooks = runtime_with(Some(observer.clone()));
    let session = SessionId::from_string("session-1").unwrap();
    let run = RunId::from_string("run-1").unwrap();

    hooks
        .observe(
            HookEventKind::SessionStarted,
            Some(&session),
            Some(&run),
            Some(std::path::Path::new("/work")),
            |_| {
                HookPayload::SessionStarted(SessionStartedPayload {
                    model: "scripted/test".into(),
                })
            },
        )
        .await;

    let seen = observer.seen.lock().unwrap();
    let envelope = seen.first().expect("one envelope was delivered");
    assert_eq!(envelope.event(), HookEventKind::SessionStarted);
    assert_eq!(envelope.identity().session_id.as_ref(), Some(&session));
    assert_eq!(envelope.identity().run_id.as_ref(), Some(&run));
    assert_eq!(
        envelope.workspace_root(),
        Some(std::path::Path::new("/work"))
    );
}

#[tokio::test]
async fn a_delegated_runtime_reports_its_parent_identity() {
    let observer = Arc::new(RecordingObserver::default());
    let parent_session = SessionId::from_string("parent-session").unwrap();
    let parent_run = RunId::from_string("parent-run").unwrap();
    let hooks = HookRuntime::new(
        Some(observer.clone()),
        None,
        HookPayloadBounds::default(),
        HookDelegation::new(parent_session.clone()).parent_run_id(parent_run.clone()),
    );
    let child = SessionId::from_string("child-session").unwrap();

    hooks
        .observe(
            HookEventKind::SessionStarted,
            Some(&child),
            None,
            None,
            |_| {
                HookPayload::SessionStarted(SessionStartedPayload {
                    model: "scripted/test".into(),
                })
            },
        )
        .await;

    let seen = observer.seen.lock().unwrap();
    assert_eq!(
        seen[0].identity(),
        &HookIdentity {
            session_id: Some(child),
            parent_session_id: Some(parent_session),
            run_id: None,
            parent_run_id: Some(parent_run),
        }
    );
}

#[tokio::test]
async fn a_runtime_without_a_gate_lets_every_request_continue() {
    let hooks = runtime_with(None);
    let envelope = crate::hooks::envelope::HookEnvelopeBuilder::new(
        HookEventKind::BeforeToolUse,
        HookIdentity::default(),
        None,
    )
    .finish(HookPayload::SessionStarted(SessionStartedPayload {
        model: "scripted/test".into(),
    }));

    let decision = hooks
        .evaluate_pre_tool_use(PreToolUseRequest::new(
            envelope,
            crate::hooks::HookPolicyOutcome::Allow,
        ))
        .await;

    assert_eq!(decision, HookDecision::Continue);
}

#[tokio::test]
async fn the_host_dispatcher_reports_both_session_boundaries() {
    let observer = Arc::new(RecordingObserver::default());
    let dispatcher =
        HookDispatcher::new(runtime_with(Some(observer.clone())), Some("/work".into()));
    let session = SessionId::from_string("session-1").unwrap();

    assert!(dispatcher.is_enabled());
    dispatcher.session_completed(&session, 4).await;
    dispatcher
        .session_failed(&session, "provider", "provider failed: overloaded")
        .await;

    assert_eq!(
        observer.events(),
        vec![
            HookEventKind::SessionCompleted,
            HookEventKind::SessionFailed
        ]
    );
    let seen = observer.seen.lock().unwrap();
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

#[tokio::test]
async fn a_dispatcher_with_only_a_gate_is_still_enabled() {
    let hooks = HookRuntime::new(
        None,
        Some(Arc::new(DenyGate)),
        HookPayloadBounds::default(),
        HookDelegation::default(),
    );

    assert!(HookDispatcher::new(hooks, None).is_enabled());
}

#[test]
fn a_dispatcher_without_hooks_reports_itself_disabled() {
    assert!(!HookDispatcher::new(HookRuntime::default(), None).is_enabled());
}
