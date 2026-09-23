use crate::tui::{
    send_confirm::PendingConfirmSend, tests::test_app, App, ComposerMode, GoalState, InlineChoice,
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

// Covers: idle subagent notifications must wait while a user prompt is queued,
// a goal owns the next turn, a turn is already running, or a confirm-send
// modal owns the exclusive SendSubmission (delivery would overwrite it).
// Owner: idle subagent delivery policy
#[test]
fn busy_app_states_block_idle_subagent_delivery() {
    let cases: [(&str, fn(&mut App)); 4] = [
        ("queued user prompt", |app| {
            app.pending.push_follow_up(queued_prompt())
        }),
        ("active goal", |app| {
            app.goal = Some(GoalState::new("finish the task".into()))
        }),
        ("running turn", App::begin_provider_turn_ui),
        ("confirm-send modal", |app| {
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
                    pending: InlineChoicePending::ConfirmSend(Box::new(
                        PendingConfirmSend::for_test(),
                    )),
                    parent_picker: None,
                }))
        }),
    ];
    for (case, setup) in cases {
        let mut app = test_app();
        setup(&mut app);
        assert!(!app.should_deliver_idle_subagent_completions(), "{case}");
    }

    // The queued prompt stays editable; clearing it unblocks delivery.
    let mut app = test_app();
    app.pending.push_follow_up(queued_prompt());
    app.should_deliver_idle_subagent_completions();
    assert_eq!(app.pending.queued_prompts().len(), 1);
    app.pending.clear_follow_ups();
    assert!(app.should_deliver_idle_subagent_completions());
}
