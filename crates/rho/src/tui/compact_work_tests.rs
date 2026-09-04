use super::{
    settle_compact_send, should_start_compact_follow_ups, CompactSettlementIntent, ReadyFollowUp,
    SettledSend,
};
use crate::tui::{send_confirm::SendSubmission, TurnPrompt};

fn submission(label: &str) -> SendSubmission {
    SendSubmission::turn(
        TurnPrompt::standard(label.to_owned(), label.to_owned()),
        Vec::new(),
        Vec::new(),
    )
}

// Covers: completed, unchanged, and failed compaction transfer one owned send
// continuation to the runnable slot; user cancellation returns that same
// continuation for cancellation instead of authorizing or queueing it.
// Owner: compact follow-up ownership
#[test]
fn compact_send_settlement_has_exactly_one_owner() {
    match settle_compact_send(Box::new(submission("proceed")), true) {
        SettledSend::Ready(ReadyFollowUp::Send(submission)) => {
            assert_eq!(submission.turn_display(), Some("proceed"));
        }
        SettledSend::Ready(ReadyFollowUp::Queued { .. }) | SettledSend::Cancelled(_) => {
            panic!("non-cancel settlement must release the exact send")
        }
    }

    match settle_compact_send(Box::new(submission("cancel")), false) {
        SettledSend::Cancelled(submission) => {
            assert_eq!(submission.turn_display(), Some("cancel"));
        }
        SettledSend::Ready(_) => panic!("user cancellation must not release the send"),
    }
}

// Covers: Esc intent wins when abort races with a compact task that already
// finished; the compact result may apply, but its follow-up must stay stopped.
// Owner: compact settlement policy
#[test]
fn user_cancellation_suppresses_follow_up_after_finished_race() {
    assert!(should_start_compact_follow_ups(
        CompactSettlementIntent::Poll,
        /*outcome_starts_follow_ups*/ true,
    ));
    assert!(!should_start_compact_follow_ups(
        CompactSettlementIntent::UserCancelled,
        /*outcome_starts_follow_ups*/ true,
    ));
}
