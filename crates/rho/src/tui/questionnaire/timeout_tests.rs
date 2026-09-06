use super::*;
use crate::tui::questionnaire::{QuestionnaireReply, QuestionnaireResponseChannel};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use pretty_assertions::assert_eq;
use rho_sdk::{
    HostChoice, HostInputRequest, HostInputResponse, HostInputSource, HostQuestion, SelectionMode,
};

// Covers: timeout opt-in, exact deadline, and permanent pause without wall-clock races.
// Owner: pure questionnaire timer policy; key routing is covered through PTY.
#[test]
fn timeout_only_submits_untouched_opted_in_forms_at_the_deadline() {
    let now = Instant::now();
    let second = NonZeroU64::new(1);
    for (seconds, fallback, event, expected) in [
        (None, true, None, false),
        (second, false, None, false),
        (
            second,
            true,
            Some(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
            false,
        ),
        (second, true, Some(Event::Paste("blue".into())), false),
        (
            second,
            true,
            Some(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })),
            false,
        ),
        (second, true, Some(Event::Resize(80, 24)), true),
        (second, true, Some(Event::FocusGained), true),
        (second, true, None, true),
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
        if let Some(event) = event {
            composer.observe_input(&event);
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
                QuestionnaireReply::Answer(
                    HostInputResponse::new()
                        .answer("color", ["blue"])
                        .with_source(HostInputSource::TimeoutFallback)
                )
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
