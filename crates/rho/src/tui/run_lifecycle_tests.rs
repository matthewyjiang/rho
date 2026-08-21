use crate::tui::{pending_input::AcceptedSteering, tests::test_app, QueuedPrompt};

fn prompt(model: &str, display: &str) -> QueuedPrompt {
    QueuedPrompt {
        prompt: model.into(),
        display_prompt: display.into(),
        paste_segments: Vec::new(),
        media: Vec::new(),
    }
}

#[test]
fn interrupt_restores_accepted_local_and_follow_up_input() {
    let mut app = test_app();
    app.input_ui.set_text("draft".to_string());
    app.input_ui.set_cursor(app.input_char_len());
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: rho_sdk::SteeringId::new(),
            prompt: prompt("accepted steer", "accepted steer"),
        });
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("local steer", "local steer"));
    app.pending
        .push_follow_up(prompt("expanded next turn", "next turn"));

    app.restore_pending_work_to_input();

    assert_eq!(
        app.input_ui.text(),
        "accepted steer\n\nlocal steer\n\nexpanded next turn\n\ndraft"
    );
    assert!(app.pending.accepted_steering().is_empty());
    assert!(app.pending.steering_prompts().is_empty());
    assert!(app.pending.queued_prompts().is_empty());
    assert_eq!(app.input_ui.cursor(), app.input_char_len());
}

#[test]
fn failed_run_preserves_unapplied_steering_as_follow_ups() {
    let mut app = test_app();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: rho_sdk::SteeringId::new(),
            prompt: prompt("accepted model", "accepted display"),
        });
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("local model", "local display"));
    app.pending
        .push_follow_up(prompt("existing model", "existing display"));

    app.preserve_unapplied_steering_as_follow_ups();

    assert!(app.pending.accepted_steering().is_empty());
    assert!(app.pending.steering_prompts().is_empty());
    assert_eq!(
        app.pending
            .queued_prompts()
            .iter()
            .map(|prompt| prompt.display_prompt.as_str())
            .collect::<Vec<_>>(),
        ["accepted display", "local display", "existing display"]
    );
}

#[test]
fn interrupt_expands_pasted_draft_before_restoring_it() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta");
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("steer", "steer"));

    app.restore_pending_work_to_input();

    assert_eq!(app.input_ui.text(), "steer\n\nalpha\nbeta");
    assert!(app.input_ui.paste_segments().is_empty());
}

// Covers: a turn that finishes while the user is scrolled up flags the jump
// chip until they return to bottom or the next turn starts.
// Owner: behavior (turn-finished attention cue)
#[test]
fn turn_finished_while_scrolled_up_flags_jump_chip() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.history.scroll_chrome_mut().set_top_line(100, 10, 0);

    app.end_busy_ui();

    assert_eq!(
        app.jump_chip_state(),
        crate::tui::activity::JumpChipState::ResponseReady
    );
}

#[test]
fn turn_finished_at_bottom_keeps_chip_neutral() {
    let mut app = test_app();
    app.begin_provider_turn_ui();

    app.end_busy_ui();

    assert_eq!(
        app.jump_chip_state(),
        crate::tui::activity::JumpChipState::Neutral
    );
}

#[test]
fn returning_to_bottom_expires_turn_finished_attention() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.history.scroll_chrome_mut().set_top_line(100, 10, 0);
    app.end_busy_ui();
    assert!(app.turn_finished_attention);

    app.history.scroll_chrome_mut().scroll_to_bottom();
    app.settle_turn_finished_attention();

    assert!(!app.turn_finished_attention);
    assert_eq!(
        app.jump_chip_state(),
        crate::tui::activity::JumpChipState::Neutral
    );
}

#[test]
fn next_turn_start_clears_turn_finished_attention() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.history.scroll_chrome_mut().set_top_line(100, 10, 0);
    app.end_busy_ui();
    assert!(app.turn_finished_attention);

    app.begin_provider_turn_ui();

    assert!(!app.turn_finished_attention);
}

// Covers: a trailing auto-compact end_busy must not wipe an unseen cue.
// Owner: behavior (turn-finished attention cue)
#[test]
fn compact_finish_does_not_clear_unseen_turn_finished_attention() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.history.scroll_chrome_mut().set_top_line(100, 10, 0);
    app.end_busy_ui();

    app.begin_compact_ui();
    app.end_busy_ui();

    assert_eq!(
        app.jump_chip_state(),
        crate::tui::activity::JumpChipState::ResponseReady
    );
}
