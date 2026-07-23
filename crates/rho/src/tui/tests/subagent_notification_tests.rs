use crate::tui::{tests::test_app, GoalState, QueuedPrompt};

fn queued_prompt() -> QueuedPrompt {
    QueuedPrompt {
        prompt: "model prompt".into(),
        display_prompt: "display prompt".into(),
        paste_segments: Vec::new(),
    }
}

#[test]
fn waiting_user_prompt_keeps_subagent_notifications_out_of_the_editable_queue() {
    let mut app = test_app();
    app.pending.queued_prompts.push_back(queued_prompt());

    assert!(!app.should_deliver_idle_subagent_completions());
    assert_eq!(app.pending.queued_prompts.len(), 1);

    app.pending.queued_prompts.clear();
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
