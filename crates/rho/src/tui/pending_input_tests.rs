use std::collections::VecDeque;

use ratatui::text::Line;

use super::*;
use crate::tui::{render::display_width, tests::test_app};

fn prompt(text: &str) -> QueuedPrompt {
    QueuedPrompt {
        prompt: text.into(),
        display_prompt: text.into(),
        paste_segments: Vec::new(),
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn panel_distinguishes_steering_from_follow_ups_and_marks_recall_target() {
    let mut app = test_app();
    app.pending
        .queued_prompts_mut()
        .push_back(prompt("run all tests"));
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: rho_sdk::SteeringId::new(),
            prompt: prompt("keep the API stable"),
        });
    app.select_pending_recall_target();

    let lines = app.pending_input_lines(80);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(text[0].contains("1 steer · 1 follow-up"));
    assert!(text[1].contains("▸ STEER"));
    assert!(text[1].contains("current run"));
    assert!(text[2].contains("NEXT"));
    assert!(text[2].contains("after turn"));
}

#[test]
fn alt_up_prioritizes_latest_local_steer_over_follow_up() {
    let mut app = test_app();
    app.pending
        .queued_prompts_mut()
        .push_back(prompt("future turn"));
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("first steer"));
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("latest steer"));

    assert!(app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::ALT)));

    assert_eq!(app.input_ui.text(), "latest steer");
    assert_eq!(
        *app.pending.steering_prompts(),
        VecDeque::from([prompt("first steer")])
    );
    assert_eq!(
        *app.pending.queued_prompts(),
        VecDeque::from([prompt("future turn")])
    );
}

#[test]
fn alt_up_requests_retraction_for_accepted_steer() {
    let mut app = test_app();
    let id = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: id.clone(),
            prompt: prompt("retract me"),
        });

    app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::ALT));

    assert!(matches!(
        app.pending.input_action(),
        Some(PendingInputAction::EditAccepted {
            id: ref action_id,
            ..
        }) if action_id == &id
    ));
    assert!(app.input_ui.text().is_empty());
    assert_eq!(app.pending.accepted_steering().len(), 1);
}

#[test]
fn alt_up_preserves_nonempty_composer() {
    let mut app = test_app();
    app.input_ui.set_text("draft".to_string());
    app.input_ui.set_cursor(app.input_char_len());
    app.pending
        .queued_prompts_mut()
        .push_back(prompt("future turn"));

    app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::ALT));

    assert_eq!(app.input_ui.text(), "draft");
    assert_eq!(
        *app.pending.queued_prompts(),
        VecDeque::from([prompt("future turn")])
    );
    assert_eq!(
        app.history.last_status_notice(),
        Some("clear the composer before editing pending input")
    );
}

#[test]
fn applied_event_preserves_selection_of_a_later_pending_item() {
    let mut app = test_app();
    let applied = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: applied.clone(),
            prompt: prompt("first steer"),
        });
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: rho_sdk::SteeringId::new(),
            prompt: prompt("second steer"),
        });
    app.pending
        .queued_prompts_mut()
        .push_back(prompt("future turn"));
    app.pending.input_panel_mut().selected = 2;

    app.mark_steering_applied(&[applied]);

    assert_eq!(app.pending.input_panel().selected, 1);
    let lines = app.pending_input_lines(80);
    assert!(line_text(&lines[2]).contains("▸ NEXT"));
}

#[test]
fn backspace_removes_the_selected_follow_up() {
    let mut app = test_app();
    app.pending.queued_prompts_mut().push_back(prompt("first"));
    app.pending.queued_prompts_mut().push_back(prompt("second"));
    app.select_pending_recall_target();

    app.handle_pending_input_key(key(KeyCode::Char('q'), KeyModifiers::ALT));
    app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::NONE));
    app.handle_pending_input_key(key(KeyCode::Backspace, KeyModifiers::NONE));

    assert_eq!(
        *app.pending.queued_prompts(),
        VecDeque::from([prompt("second")])
    );
}

#[test]
fn rejected_steering_acceptance_becomes_a_follow_up_without_failing_the_turn() {
    let mut app = test_app();
    let queued = prompt("continue after this turn");
    let request = PendingInputRequest::Accept {
        prompt: queued.clone(),
        receipt: Box::pin(std::future::pending()),
    };
    let completion = PendingInputCompletion::Accepted(Err(rho_sdk::Error::InvalidHostResponse {
        message: "run completed before accepting steering input".into(),
    }));

    let failure = app.finish_pending_input_request(request, completion);

    assert_eq!(failure, None);
    assert!(app.pending.steering_prompts().is_empty());
    assert_eq!(*app.pending.queued_prompts(), VecDeque::from([queued]));
    assert_eq!(
        app.history.last_status_notice(),
        Some(
            "steer queued as follow-up: invalid host response: run completed before accepting steering input"
        )
    );
}

#[test]
fn applied_event_removes_only_matching_steering() {
    let mut app = test_app();
    let applied = rho_sdk::SteeringId::new();
    let pending = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: applied.clone(),
            prompt: prompt("applied"),
        });
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: pending.clone(),
            prompt: prompt("pending"),
        });

    app.mark_steering_applied(&[applied]);

    assert_eq!(app.pending.accepted_steering().len(), 1);
    assert_eq!(app.pending.accepted_steering()[0].id, pending);
}

#[test]
fn panel_reserves_space_immediately_above_composer() {
    let mut app = test_app();
    app.input_ui.set_text("draft".to_string());
    app.input_ui.set_cursor(app.input_char_len());
    app.pending
        .queued_prompts_mut()
        .push_back(prompt("future turn"));
    app.select_pending_recall_target();

    let layout = app.screen_layout(
        ratatui::layout::Rect::new(0, 0, 80, 24),
        std::time::Instant::now(),
    );

    assert!(layout.pending_input.height > 0);
    assert_eq!(layout.pending_input.y, layout.history.bottom());
    assert_eq!(layout.top_divider.y, layout.pending_input.bottom());
    assert_eq!(layout.composer.y, layout.top_divider.bottom());
    assert!(layout.composer.height > 0);
}

#[test]
fn focused_panel_stays_visible_with_a_tall_composer_in_a_short_terminal() {
    let mut app = test_app();
    app.input_ui.set_text(
        "a long draft that wraps across many composer lines in a narrow terminal".to_string(),
    );
    app.input_ui.set_cursor(app.input_char_len());
    app.pending
        .queued_prompts_mut()
        .push_back(prompt("future turn"));
    app.pending.input_panel_mut().focused = true;
    app.select_pending_recall_target();

    let layout = app.screen_layout(
        ratatui::layout::Rect::new(0, 0, 24, 8),
        std::time::Instant::now(),
    );

    assert!(layout.pending_input.height >= 2);
    assert!(layout.composer.height >= 1);
}

#[test]
fn panel_lines_fit_narrow_terminal() {
    let mut app = test_app();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: rho_sdk::SteeringId::new(),
            prompt: prompt("a long steering prompt that must be truncated"),
        });
    app.select_pending_recall_target();

    for width in 1..40 {
        assert!(app
            .pending_input_lines(width)
            .iter()
            .all(|line| display_width(&line_text(line)) <= width));
    }
}
