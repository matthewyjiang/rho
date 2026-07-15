use super::*;
use crate::tui::tests::test_app;

fn failed_turn() -> FailedTurn {
    FailedTurn {
        input: rho_sdk::UserInput::text("continue the existing goal turn"),
        display_user: Some(Message::user_text("continuing active goal")),
    }
}

#[test]
fn retry_request_reuses_the_failed_turn_input_and_display() {
    let failed_turn = failed_turn();
    let PromptTurnRequest::Retry(retry) = PromptTurnRequest::Retry(failed_turn.clone()) else {
        unreachable!("constructed a retry request")
    };

    assert_eq!(retry, failed_turn);
}

#[test]
fn failed_turn_keeps_live_partial_assistant_text_before_error() {
    let mut app = test_app();
    app.running = true;
    app.current_turn_start = Some(0);
    app.assistant_stream
        .push_delta("partial assistant before stream failure");

    let outcome = app.finalize_failed_turn("provider stream failed".into(), failed_turn());

    assert_eq!(outcome.kind(), TurnOutcomeKind::Failed);
    assert!(matches!(
        app.transcript.as_slice(),
        [Entry::Assistant(text), Entry::Error(error)]
            if text == "partial assistant before stream failure"
                && error == "provider stream failed"
    ));
    assert!(!app.running);
    assert!(app.assistant_stream.is_empty());
    assert_eq!(app.status, "error");
}
