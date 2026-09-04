use crate::tui::{
    send_confirm::PendingConfirmSend, tests::test_app, ComposerMode, GoalState, InlineChoice,
    InlineChoiceModal, InlineChoiceOption, InlineChoicePending, QueuedPrompt,
};

fn queued_prompt() -> QueuedPrompt {
    QueuedPrompt {
        prompt: "model prompt".into(),
        display_prompt: "display prompt".into(),
        paste_segments: Vec::new(),
        media: Vec::new(),
    }
}

#[test]
fn waiting_user_prompt_keeps_subagent_notifications_out_of_the_editable_queue() {
    let mut app = test_app();
    app.pending.push_follow_up(queued_prompt());

    assert!(!app.should_deliver_idle_subagent_completions());
    assert_eq!(app.pending.queued_prompts().len(), 1);

    app.pending.clear_follow_ups();
    assert!(app.should_deliver_idle_subagent_completions());
}

#[test]
fn active_goal_keeps_subagent_notifications_for_the_goal_turn() {
    let mut app = test_app();
    app.goal = Some(GoalState::new("finish the task".into()));

    assert!(!app.should_deliver_idle_subagent_completions());
}

#[test]
fn running_turn_cannot_start_synthetic_notification_delivery() {
    let mut app = test_app();
    app.begin_provider_turn_ui();

    assert!(!app.should_deliver_idle_subagent_completions());
}

// Covers: idle subagent delivery must not overwrite a confirm-send modal and
// drop the exclusive SendSubmission it owns.
// Owner: send confirmation ownership
#[test]
fn confirm_send_modal_blocks_idle_subagent_delivery() {
    let mut app = test_app();
    assert!(app.should_deliver_idle_subagent_completions());

    app.input_ui
        .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
            choice: InlineChoice::new(
                "Send to openai/gpt-5?",
                "native context",
                vec![InlineChoiceOption::available(
                    "send",
                    '1',
                    "Send anyway",
                    "detail",
                )],
            )
            .unwrap(),
            pending: InlineChoicePending::ConfirmSend(Box::new(PendingConfirmSend::for_test())),
            parent_picker: None,
        }));

    assert!(!app.should_deliver_idle_subagent_completions());
}
