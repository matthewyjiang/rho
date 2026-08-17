use pretty_assertions::assert_eq;

use super::super::{
    prompt_history_persistence::{PromptHistoryOp, MAX_PERSISTED_PROMPT_BYTES},
    tests::test_app,
    CommandId, CommandInvocation, HistoryDirection,
};

fn prompt_history_invocation(args: &str) -> CommandInvocation {
    CommandInvocation {
        id: CommandId::PromptHistory,
        name: "prompt-history".into(),
        raw_args: format!(" {args}"),
        args: args.to_string(),
    }
}

// Covers: /prompt-history clear wipes the ring and enqueues Clear after the
// command's own history append so the durable log ends empty.
// Owner: tui prompt-history command
#[test]
fn clear_resets_ring_and_enqueues_nuclear_clear() {
    let mut app = test_app();
    app.input_ui.set_text("/prompt-history clear".into());
    let invocation = app.parse_input_command().unwrap().unwrap();
    assert_eq!(invocation.id, CommandId::PromptHistory);
    assert_eq!(
        app.input_ui.history(),
        &["/prompt-history clear".to_string()]
    );

    app.execute_prompt_history_command(invocation).unwrap();

    assert!(app.input_ui.history().is_empty());
    assert_eq!(app.input_ui.history_cursor(), None);
    let mut rx = app.take_prompt_history_rx().unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        PromptHistoryOp::Append("/prompt-history clear".into())
    );
    assert_eq!(rx.try_recv().unwrap(), PromptHistoryOp::Clear);
    assert!(rx.try_recv().is_err());
}

// Covers: unknown prompt-history args do not wipe history.
// Owner: tui prompt-history command
#[test]
fn unknown_args_leave_history_untouched() {
    let mut app = test_app();
    app.push_input_history("keep me");
    app.execute_prompt_history_command(prompt_history_invocation("nope"))
        .unwrap();
    assert_eq!(app.input_ui.history(), &["keep me".to_string()]);
}

// Covers: oversized sent text stays in the in-memory ring but is not persisted.
// Owner: tui prompt-history persistence policy
#[test]
fn oversized_prompt_stays_in_ring_only() {
    let mut app = test_app();
    let oversized = "a".repeat(MAX_PERSISTED_PROMPT_BYTES + 1);
    app.push_input_history(&oversized);
    assert_eq!(app.input_ui.history(), &[oversized]);
    let mut rx = app.take_prompt_history_rx().unwrap();
    assert!(rx.try_recv().is_err());
}

// Covers: seeding older entries in front of local history preserves recall
// cursor so an in-progress up-arrow does not jump.
// Owner: tui input history
#[test]
fn seed_history_front_offsets_in_progress_recall() {
    let mut app = test_app();
    app.push_input_history("local");
    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "local");
    assert_eq!(app.input_ui.history_cursor(), Some(0));

    app.input_ui
        .seed_history_front(vec!["older".into(), "local".into()]);
    assert_eq!(
        app.input_ui.history(),
        &["older".to_string(), "local".to_string()]
    );
    assert_eq!(app.input_ui.history_cursor(), Some(1));
    assert_eq!(app.input_ui.text(), "local");

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "older");
}
