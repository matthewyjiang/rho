use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity},
    provider::{ScriptedProvider, ScriptedTurn},
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CancellationToken,
    CapabilityRequest, CapabilitySource, PathScope, Rho, SessionOptions,
};

use super::{ClassificationInput, ClassifierApprovalHandler, ClassifyFn};
use crate::permission_classifier::{ClassifierVerdict, CONSECUTIVE_DENY_ESCALATION};

fn request() -> ApprovalRequest {
    ApprovalRequest::new(
        CapabilityRequest::write_path(
            "/workspace/file.txt",
            PathScope::PrimaryWorkspace,
            CapabilitySource::built_in_tool("write"),
        ),
        "approval required",
    )
}

#[derive(Clone)]
struct ScriptedClassifier {
    outcomes: Arc<Mutex<VecDeque<anyhow::Result<ClassifierVerdict>>>>,
    calls: Arc<Mutex<Vec<ApprovalRequest>>>,
}

impl ScriptedClassifier {
    fn new(outcomes: impl IntoIterator<Item = anyhow::Result<ClassifierVerdict>>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
            calls: Arc::default(),
        }
    }

    fn classify(&self) -> ClassifyFn {
        let outcomes = Arc::clone(&self.outcomes);
        let calls = Arc::clone(&self.calls);
        Arc::new(move |input: ClassificationInput| {
            calls.lock().unwrap().push(input.request.clone());
            let outcome = outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted classifier outcome");
            Box::pin(std::future::ready(outcome))
        })
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[derive(Clone)]
struct ScriptedApprovals {
    decisions: Arc<Mutex<VecDeque<ApprovalDecision>>>,
    requests: Arc<Mutex<Vec<ApprovalRequest>>>,
}

impl ScriptedApprovals {
    fn new(decisions: impl IntoIterator<Item = ApprovalDecision>) -> Self {
        Self {
            decisions: Arc::new(Mutex::new(decisions.into_iter().collect())),
            requests: Arc::default(),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl ApprovalHandler for ScriptedApprovals {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            self.decisions
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted approval decision")
        })
    }
}

fn handler_with(
    classifier: &ScriptedClassifier,
    inner: Option<Arc<dyn ApprovalHandler>>,
) -> ClassifierApprovalHandler {
    ClassifierApprovalHandler::for_tests(classifier.classify(), inner)
}

// Covers: forked handlers keep distinct session bindings so parallel workflow
// agents cannot overwrite each other's classifier transcript.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn fork_unbound_isolates_session_bindings() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let classifier: ClassifyFn = {
        let observed = Arc::clone(&observed);
        Arc::new(move |input: ClassificationInput| {
            observed.lock().unwrap().push(input.history.clone());
            Box::pin(std::future::ready(Ok(ClassifierVerdict::Allow)))
        })
    };
    let template = Arc::new(ClassifierApprovalHandler::for_tests(classifier, None));
    let first = template.fork_unbound();
    let second = template.fork_unbound();

    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "unused"),
            Vec::<ScriptedTurn>::new(),
        ))
        .build()
        .unwrap();
    let first_history = vec![Message::user_text("first agent")];
    let second_history = vec![Message::user_text("second agent")];
    let first_session = runtime
        .session(SessionOptions::default().history(first_history.clone()))
        .await
        .unwrap();
    let second_session = runtime
        .session(SessionOptions::default().history(second_history.clone()))
        .await
        .unwrap();
    first.bind_session(first_session);
    second.bind_session(second_session);

    assert_eq!(first.request(request()).await, ApprovalDecision::AllowOnce);
    assert_eq!(second.request(request()).await, ApprovalDecision::AllowOnce);

    assert_eq!(
        *observed.lock().unwrap(),
        vec![first_history, second_history]
    );
    runtime.shutdown();
}

// Covers: workflow Auto agent classifiers need the child session history instead of empty context.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn bound_session_history_reaches_classifier_input() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let classifier: ClassifyFn = {
        let observed = Arc::clone(&observed);
        Arc::new(move |input: ClassificationInput| {
            observed.lock().unwrap().push(input.history.clone());
            Box::pin(std::future::ready(Ok(ClassifierVerdict::Allow)))
        })
    };
    let handler = ClassifierApprovalHandler::for_tests(classifier, None);
    let history = vec![Message::user_text("prior workflow context")];
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "unused"),
            Vec::<ScriptedTurn>::new(),
        ))
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::default().history(history.clone()))
        .await
        .unwrap();
    handler.bind_session(session);

    assert_eq!(
        handler.request(request()).await,
        ApprovalDecision::AllowOnce
    );

    assert_eq!(*observed.lock().unwrap(), vec![history]);
    runtime.shutdown();
}

// Covers: in-flight classifier calls must share the bound run cancellation token.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn classifier_input_uses_bound_cancellation_token() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let classifier: ClassifyFn = {
        let observed = Arc::clone(&observed);
        Arc::new(move |input: ClassificationInput| {
            observed
                .lock()
                .unwrap()
                .push(input.cancellation.is_cancelled());
            Box::pin(std::future::ready(Ok(ClassifierVerdict::Allow)))
        })
    };
    let handler = ClassifierApprovalHandler::for_tests(classifier, None);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    handler.bind_cancellation(cancellation);

    assert_eq!(
        handler.request(request()).await,
        ApprovalDecision::AllowOnce
    );

    assert_eq!(*observed.lock().unwrap(), vec![true]);
}

// Covers: classifier allows should not grant session-wide approval and should clear deny streaks.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn allow_returns_allow_once_and_resets_consecutive_denials() {
    let classifier = ScriptedClassifier::new([
        Ok(ClassifierVerdict::Deny {
            reason: "too broad".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "still too broad".into(),
        }),
        Ok(ClassifierVerdict::Allow),
        Ok(ClassifierVerdict::Deny {
            reason: "new streak one".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "new streak two".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "new streak three".into(),
        }),
    ]);
    let inner = Arc::new(ScriptedApprovals::new([ApprovalDecision::AllowOnce]));
    let handler = handler_with(&classifier, Some(inner.clone()));

    assert!(matches!(
        handler.request(request()).await,
        ApprovalDecision::Deny { .. }
    ));
    assert!(matches!(
        handler.request(request()).await,
        ApprovalDecision::Deny { .. }
    ));
    assert_eq!(
        handler.request(request()).await,
        ApprovalDecision::AllowOnce
    );
    for _ in 0..CONSECUTIVE_DENY_ESCALATION {
        assert!(matches!(
            handler.request(request()).await,
            ApprovalDecision::Deny { .. }
        ));
    }

    assert_eq!(classifier.call_count(), 6);
    assert_eq!(inner.request_count(), 0);
}

// Covers: after three classifier denials, interactive Auto escalates to the human channel.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn after_three_denials_next_request_escalates_to_inner_handler_and_resets() {
    let classifier = ScriptedClassifier::new([
        Ok(ClassifierVerdict::Deny {
            reason: "one".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "two".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "three".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "after reset".into(),
        }),
    ]);
    let inner = Arc::new(ScriptedApprovals::new([ApprovalDecision::AllowOnce]));
    let handler = handler_with(&classifier, Some(inner.clone()));

    for _ in 0..CONSECUTIVE_DENY_ESCALATION {
        assert!(matches!(
            handler.request(request()).await,
            ApprovalDecision::Deny { .. }
        ));
    }
    assert_eq!(
        handler.request(request()).await,
        ApprovalDecision::AllowOnce
    );
    assert!(matches!(
        handler.request(request()).await,
        ApprovalDecision::Deny { .. }
    ));

    assert_eq!(classifier.call_count(), 4);
    assert_eq!(inner.request_count(), 1);
}

// Covers: classifier transport/parse failures fail closed and count toward headless escalation.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn classifier_errors_deny_and_headless_escalation_does_not_call_classifier() {
    let classifier = ScriptedClassifier::new([
        Err(anyhow::anyhow!("timeout")),
        Err(anyhow::anyhow!("malformed verdict")),
        Err(anyhow::anyhow!("provider unavailable")),
    ]);
    let handler = handler_with(&classifier, None);

    for _ in 0..CONSECUTIVE_DENY_ESCALATION {
        let decision = handler.request(request()).await;
        let ApprovalDecision::Deny { reason } = decision else {
            panic!("classifier failures must deny");
        };
        assert!(reason.contains("find a safer path"));
        assert!(reason.contains("do not route around this block"));
    }
    let decision = handler.request(request()).await;
    let ApprovalDecision::Deny { reason } = decision else {
        panic!("headless escalation must deny");
    };
    assert!(reason.contains("permission classifier denied 3 consecutive requests"));

    assert_eq!(classifier.call_count(), 3);
}

// Covers: headless Auto must fail the run after repeated classifier denials
// instead of denying forever while automation keeps running.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn headless_escalation_cancels_bound_run_token() {
    let classifier = ScriptedClassifier::new([
        Ok(ClassifierVerdict::Deny {
            reason: "one".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "two".into(),
        }),
        Ok(ClassifierVerdict::Deny {
            reason: "three".into(),
        }),
    ]);
    let handler = handler_with(&classifier, None);
    let cancellation = CancellationToken::new();
    handler.bind_cancellation(cancellation.clone());

    for _ in 0..CONSECUTIVE_DENY_ESCALATION {
        assert!(matches!(
            handler.request(request()).await,
            ApprovalDecision::Deny { .. }
        ));
        assert!(!cancellation.is_cancelled());
    }

    let decision = handler.request(request()).await;
    let ApprovalDecision::Deny { reason } = decision else {
        panic!("headless escalation must deny");
    };
    assert!(reason.contains("permission classifier denied 3 consecutive requests"));
    assert!(cancellation.is_cancelled());
    assert_eq!(classifier.call_count(), 3);
}
