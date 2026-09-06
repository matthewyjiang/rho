use super::*;
use crate::tui::questionnaire::{QuestionnaireReply, QuestionnaireResponseChannel};
use pretty_assertions::assert_eq;
use rho_sdk::{
    HostChoice, HostInputRequest, HostInputResponse, HostInputSource, HostQuestion, SelectionMode,
};

// Covers: timeout opt-in, exact deadline, and permanent pause without wall-clock races.
// Owner: pure questionnaire timer policy; key routing is covered through PTY.
#[test]
fn timeout_only_submits_untouched_opted_in_forms_at_the_deadline() {
    let now = Instant::now();
    for (seconds, fallback, pause, expected) in [
        (None, true, false, false),
        (NonZeroU64::new(1), false, false, false),
        (NonZeroU64::new(1), true, true, false),
        (NonZeroU64::new(1), true, false, true),
    ] {
        let mut request = HostInputRequest::questionnaire(
            "Color",
            vec![HostQuestion::new(
                "color",
                "Color?",
                vec![HostChoice::new("blue", "Blue")],
                SelectionMode::One,
            )
            .unwrap()],
        )
        .unwrap();
        if fallback {
            request = request
                .with_timeout_fallback(
                    HostInputResponse::new().answer("color", ["blue"]),
                    "Use blue",
                )
                .unwrap();
        }
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let mut composer =
            QuestionnaireComposer::new(request, QuestionnaireResponseChannel::new(tx));
        composer.start_timeout(seconds, now);
        assert_eq!(composer.submit_timeout_if_due(now), None);
        if pause {
            composer.pause_timeout();
        }
        assert_eq!(
            composer
                .submit_timeout_if_due(now + Duration::from_secs(1))
                .is_some(),
            expected
        );
        if expected {
            assert_eq!(
                rx.try_recv().unwrap(),
                QuestionnaireReply::Answer(QuestionnaireResponse {
                    answers: vec![QuestionnaireAnswer {
                        id: "color".into(),
                        answer: serde_json::json!(["blue"])
                    }],
                    source: HostInputSource::TimeoutFallback,
                })
            );
        } else {
            assert!(rx.try_recv().is_err());
        }
        assert_eq!(
            composer.submit_timeout_if_due(now + Duration::from_secs(3600)),
            None
        );
    }
}
