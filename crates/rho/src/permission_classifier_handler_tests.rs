use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalContext, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CancellationToken, CapabilityRequest, CapabilitySource, PathScope, SessionId,
};

use super::{
    ClassificationInput, ClassifierApprovalHandler, ClassifyFn, CONSECUTIVE_DENY_ESCALATION,
};
use crate::permission_classifier::ClassifierVerdict;

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

fn context_with(
    history: Vec<rho_sdk::model::Message>,
    cancellation: CancellationToken,
) -> ApprovalContext {
    ApprovalContext::new(SessionId::new(), cancellation, history)
}

#[derive(Clone)]
struct ScriptedClassifier {
    outcomes: Arc<Mutex<VecDeque<ClassifierVerdict>>>,
    calls: Arc<Mutex<Vec<ApprovalRequest>>>,
    histories: Arc<Mutex<Vec<Vec<rho_sdk::model::Message>>>>,
    cancelled: Arc<Mutex<Vec<bool>>>,
}

impl ScriptedClassifier {
    fn new(outcomes: impl IntoIterator<Item = ClassifierVerdict>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
            calls: Arc::default(),
            histories: Arc::default(),
            cancelled: Arc::default(),
        }
    }

    fn classify(&self) -> ClassifyFn {
        let outcomes = Arc::clone(&self.outcomes);
        let calls = Arc::clone(&self.calls);
        let histories = Arc::clone(&self.histories);
        let cancelled = Arc::clone(&self.cancelled);
        Arc::new(move |input: ClassificationInput| {
            calls.lock().unwrap().push(input.request.clone());
            histories
                .lock()
                .unwrap()
                .push(input.request.context().history().to_vec());
            cancelled
                .lock()
                .unwrap()
                .push(input.request.context().cancellation().is_cancelled());
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

// Covers: isolated handlers keep distinct deny streaks for parallel workflow agents.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn isolate_resets_deny_streak_without_sharing_counters() {
    let classifier = ScriptedClassifier::new([
        ClassifierVerdict::Deny {
            reason: "one".into(),
        },
        ClassifierVerdict::Deny {
            reason: "two".into(),
        },
        ClassifierVerdict::Deny {
            reason: "three".into(),
        },
        ClassifierVerdict::Allow,
    ]);
    let template = Arc::new(ClassifierApprovalHandler::for_tests(
        classifier.classify(),
        None,
    ));
    let first = template.isolate();
    let second = template.isolate();

    for _ in 0..CONSECUTIVE_DENY_ESCALATION {
        assert!(matches!(
            first.request(request()).await,
            ApprovalDecision::Deny { .. }
        ));
    }
    // First is escalated; second still has a fresh streak and can allow.
    assert_eq!(second.request(request()).await, ApprovalDecision::AllowOnce);
    assert_eq!(classifier.call_count(), 4);
}

// Covers: workflow Auto agent classifiers need the child session history from ApprovalContext.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn approval_context_history_reaches_classifier_input() {
    let classifier = ScriptedClassifier::new([ClassifierVerdict::Allow]);
    let handler = ClassifierApprovalHandler::for_tests(classifier.classify(), None);
    let history = vec![rho_sdk::model::Message::user_text("prior workflow context")];

    assert_eq!(
        handler
            .request(
                request().with_context(context_with(history.clone(), CancellationToken::new(),))
            )
            .await,
        ApprovalDecision::AllowOnce
    );

    assert_eq!(*classifier.histories.lock().unwrap(), vec![history]);
}

// Covers: in-flight classifier calls must share the run cancellation token from ApprovalContext.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn approval_context_cancellation_reaches_classifier_input() {
    let classifier = ScriptedClassifier::new([ClassifierVerdict::Allow]);
    let handler = ClassifierApprovalHandler::for_tests(classifier.classify(), None);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    assert_eq!(
        handler
            .request(request().with_context(context_with(Vec::new(), cancellation)))
            .await,
        ApprovalDecision::AllowOnce
    );

    assert_eq!(*classifier.cancelled.lock().unwrap(), vec![true]);
}

// Covers: classifier allows should not grant session-wide approval and should clear deny streaks.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn allow_returns_allow_once_and_resets_consecutive_denials() {
    let classifier = ScriptedClassifier::new([
        ClassifierVerdict::Deny {
            reason: "too broad".into(),
        },
        ClassifierVerdict::Deny {
            reason: "still too broad".into(),
        },
        ClassifierVerdict::Allow,
        ClassifierVerdict::Deny {
            reason: "new streak one".into(),
        },
        ClassifierVerdict::Deny {
            reason: "new streak two".into(),
        },
        ClassifierVerdict::Deny {
            reason: "new streak three".into(),
        },
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
        ClassifierVerdict::Deny {
            reason: "one".into(),
        },
        ClassifierVerdict::Deny {
            reason: "two".into(),
        },
        ClassifierVerdict::Deny {
            reason: "three".into(),
        },
        ClassifierVerdict::Deny {
            reason: "after reset".into(),
        },
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

// Covers: classifier unavailable denials count toward headless escalation.
// Owner: permission classifier approval handler.
#[tokio::test]
async fn unavailable_denials_escalate_headless_without_further_classifier_calls() {
    let classifier = ScriptedClassifier::new([
        ClassifierVerdict::Deny {
            reason: "classifier unavailable".into(),
        },
        ClassifierVerdict::Deny {
            reason: "classifier unavailable".into(),
        },
        ClassifierVerdict::Deny {
            reason: "classifier unavailable".into(),
        },
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
async fn headless_escalation_cancels_context_run_token() {
    let classifier = ScriptedClassifier::new([
        ClassifierVerdict::Deny {
            reason: "one".into(),
        },
        ClassifierVerdict::Deny {
            reason: "two".into(),
        },
        ClassifierVerdict::Deny {
            reason: "three".into(),
        },
    ]);
    let handler = handler_with(&classifier, None);
    let cancellation = CancellationToken::new();

    for _ in 0..CONSECUTIVE_DENY_ESCALATION {
        assert!(matches!(
            handler
                .request(request().with_context(context_with(Vec::new(), cancellation.clone(),)))
                .await,
            ApprovalDecision::Deny { .. }
        ));
        assert!(!cancellation.is_cancelled());
    }

    let decision = handler
        .request(request().with_context(context_with(Vec::new(), cancellation.clone())))
        .await;
    let ApprovalDecision::Deny { reason } = decision else {
        panic!("headless escalation must deny");
    };
    assert!(reason.contains("permission classifier denied 3 consecutive requests"));
    assert!(cancellation.is_cancelled());
    assert_eq!(classifier.call_count(), 3);
}

// Covers: Auto classifier must declare live-history reads so the SDK publishes
// in-flight transcript without a host force-publish flag.
// Owner: permission classifier approval handler.
#[test]
fn classifier_handler_reads_live_history() {
    let handler = ClassifierApprovalHandler::for_tests(
        Arc::new(|_: ClassificationInput| Box::pin(async { ClassifierVerdict::Allow })),
        None,
    );
    assert!(handler.reads_live_history());
}
