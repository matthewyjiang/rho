use pretty_assertions::assert_eq;
use rho_sdk::{model::Message, Revision};

use super::PendingTurn;

// Covers: stopping before a queued compaction event is consumed must not lose
// a boundary receipt already acknowledged against the replacement history.
#[test]
fn accepted_receipt_survives_an_unconsumed_compaction_event() {
    let human = Message::user_text("human prompt");
    let boundary = Message::user_text("boundary");
    let receipt = Message::System("receipt".into());
    let mut turn = PendingTurn::new(human.clone(), None, /*history_start*/ 0);
    let mut history = vec![
        Message::System("compacted summary".into()),
        boundary.clone(),
    ];
    turn.record_boundary_display(boundary, receipt.clone(), &history, Revision::from_u64(1));
    let tail = Message::assistant_text("retained tail");
    history.push(tail.clone());
    assert_eq!(
        turn.display_tail(&history, None),
        vec![human, receipt, tail]
    );
}

// Covers: accepted displays replace only their captured typed occurrence.
// Owner: interactive persistence mapping, independent of presentation policy.
#[test]
fn boundary_displays_bind_to_distinct_typed_history_occurrences() {
    let input = Message::user_text("same input");
    let first_display = Message::System("first display".into());
    let second_display = Message::System("second display".into());
    let mut history = vec![input.clone(), input.clone(), input.clone()];
    let mut turn = PendingTurn::new(
        input.clone(),
        /*display_user*/ None,
        /*history_start*/ 1,
    );
    turn.record_boundary_display(
        input.clone(),
        first_display.clone(),
        &history,
        Revision::from_u64(1),
    );
    turn.record_boundary_display(
        input.clone(),
        first_display.clone(),
        &history,
        Revision::from_u64(1),
    );
    history.push(Message::assistant_text("intermediate"));
    history.push(input.clone());
    turn.record_boundary_display(
        input.clone(),
        second_display.clone(),
        &history,
        Revision::from_u64(2),
    );
    history.push(input.clone());
    let model_history = history.clone();

    assert_eq!(
        turn.display_tail(&history, None),
        vec![
            input.clone(),
            first_display,
            Message::assistant_text("intermediate"),
            second_display,
            input
        ]
    );
    assert_eq!(history, model_history);
}

// Covers: compaction cannot discard captured receipts or the later untouched
// steering, tool and assistant tail, even when its event arrives after acceptance.
// Owner: interactive persistence checkpoints. SDK history is supplied explicitly
// to exercise both delivery orders without scheduler timing.
#[test]
fn compaction_checkpoints_preserve_accepted_prefix_and_later_tail() {
    for delayed_compaction_event in [false, true] {
        let input = Message::user_text("model start");
        let human = Message::user_text("human start");
        let initial_receipt = Message::System("initial receipt".into());
        let first = Message::user_text("first boundary");
        let first_receipt = Message::System("first receipt".into());
        let second = Message::user_text("second boundary");
        let second_receipt = Message::System("second receipt".into());
        let intermediate = Message::assistant_text("intermediate");
        let mut turn = PendingTurn::new(
            input.clone(),
            Some(vec![human.clone(), initial_receipt.clone()]),
            /*history_start*/ 0,
        );
        turn.record_boundary_display(
            first.clone(),
            first_receipt.clone(),
            &[input, intermediate.clone(), first],
            Revision::from_u64(1),
        );
        let compacted = vec![Message::System("summary".into())];
        let mut history = compacted.clone();
        history.push(Message::user_text("steering"));
        history.push(second.clone());
        let prefix = vec![human, initial_receipt, intermediate, first_receipt];
        if !delayed_compaction_event {
            assert_eq!(
                turn.checkpoint_compaction(&compacted, Revision::from_u64(2)),
                prefix
            );
        }
        turn.record_boundary_display(
            second,
            second_receipt.clone(),
            &history,
            Revision::from_u64(3),
        );
        if delayed_compaction_event {
            assert_eq!(
                turn.checkpoint_compaction(&compacted, Revision::from_u64(2)),
                prefix
            );
        }
        let tail = vec![
            Message::user_text("steering"),
            second_receipt,
            Message::ToolResult(rho_sdk::model::ToolResult {
                id: "call".into(),
                content: "tool output".into(),
                ok: true,
            }),
            Message::assistant_text("done"),
        ];
        history.extend_from_slice(&tail[2..]);
        let mut expected = prefix;
        expected.extend(tail);
        assert_eq!(turn.display_tail(&history, None), expected);
        // A second compaction saves the entire accumulated display once; later
        // repeated input is ordinary user content, not a reused receipt mapping.
        let next_compaction = compacted;
        // Capture the completed prefix through a final boundary before compacting.
        let final_input = Message::user_text("final boundary");
        let final_receipt = Message::System("final receipt".into());
        history.push(final_input.clone());
        turn.record_boundary_display(
            final_input.clone(),
            final_receipt.clone(),
            &history,
            Revision::from_u64(4),
        );
        expected.push(final_receipt);
        assert_eq!(
            turn.checkpoint_compaction(&next_compaction, Revision::from_u64(5)),
            expected
        );
        let mut final_history = next_compaction;
        final_history.push(final_input.clone());
        expected.push(final_input);
        assert_eq!(turn.display_tail(&final_history, None), expected);
    }
}
