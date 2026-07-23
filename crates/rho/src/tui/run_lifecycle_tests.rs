use crate::tui::{pending_input::AcceptedSteering, tests::test_app, QueuedPrompt};

fn prompt(model: &str, display: &str) -> QueuedPrompt {
    QueuedPrompt {
        prompt: model.into(),
        display_prompt: display.into(),
        paste_segments: Vec::new(),
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
        .queued_prompts_mut()
        .push_back(prompt("expanded next turn", "next turn"));

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
        .queued_prompts_mut()
        .push_back(prompt("existing model", "existing display"));

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
