use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::SubagentInbox;
use crate::app::subagent_messaging::{
    NoticePostError, SubagentNotice, SubagentNoticeBridge, NOTICE_QUEUE_CAPACITY,
};

// Covers: a notice addressed to a session the parent has left is dropped, not
// requeued. A retained one would keep the parent looking busy for the rest of
// the process and pin the idle event loop to its short poll interval.
// Owner: tui subagent inbox
#[test]
fn notices_for_a_departed_parent_session_are_discarded() {
    let current = SessionId::from_string("session-current").unwrap();
    let departed = SessionId::from_string("session-departed").unwrap();
    let mut inbox = SubagentInbox::default();
    inbox.push_notice_for_test(notice("a1", &departed));
    inbox.push_notice_for_test(notice("b2", &current));

    assert!(inbox.discard_stale(&current));
    assert!(inbox.has_pending_notices());

    let taken = inbox.take_notices(&current);
    assert_eq!(
        taken.iter().map(|n| n.run_id.as_str()).collect::<Vec<_>>(),
        vec!["b2"]
    );
    assert!(!inbox.has_pending_notices());
}

// Covers: taking notices never leaves an undeliverable one behind, even when
// discard_stale has not run for that session yet.
// Owner: tui subagent inbox
#[test]
fn taking_notices_drops_the_ones_it_cannot_deliver() {
    let current = SessionId::from_string("session-current").unwrap();
    let departed = SessionId::from_string("session-departed").unwrap();
    let mut inbox = SubagentInbox::default();
    inbox.push_notice_for_test(notice("a1", &departed));

    assert!(inbox.take_notices(&current).is_empty());
    assert!(!inbox.has_pending_notices());
}

// Covers: a failed provider start can put taken notices back ahead of newer
// arrivals so turn-boundary delivery keeps original order.
// Owner: tui subagent inbox
#[test]
fn returned_notices_preserve_order_at_the_front() {
    let current = SessionId::from_string("session-current").unwrap();
    let mut inbox = SubagentInbox::default();
    inbox.push_notice_for_test(notice("a1", &current));
    inbox.push_notice_for_test(notice("b2", &current));
    let taken = inbox.take_notices(&current);
    inbox.push_notice_for_test(notice("c3", &current));
    inbox.return_notices(taken);
    let ordered = inbox
        .take_notices(&current)
        .into_iter()
        .map(|notice| notice.run_id)
        .collect::<Vec<_>>();
    assert_eq!(
        ordered,
        vec!["a1".to_owned(), "b2".to_owned(), "c3".to_owned()]
    );
}

// Covers: draining the transport into the TUI pending queue must not bypass
// NOTICE_QUEUE_CAPACITY. message_parent keeps failing loud until delivery or
// discard frees an end-to-end slot.
// Owner: tui subagent inbox + notice bridge
#[test]
fn draining_into_pending_queue_keeps_end_to_end_notice_capacity() {
    let bridge = SubagentNoticeBridge::new();
    let receiver = bridge.bind_parent();
    let mut inbox = SubagentInbox::default();
    inbox.bind_notices_for_test(receiver, bridge.permits());
    let session = SessionId::from_string("session-1").unwrap();

    for index in 0..NOTICE_QUEUE_CAPACITY {
        bridge
            .post(notice(&format!("n{index}"), &session))
            .expect("queue should accept up to capacity");
    }
    assert!(inbox.drain(), "channel notices move into the pending queue");
    assert_eq!(inbox.queued_notice_count(), NOTICE_QUEUE_CAPACITY);
    assert_eq!(
        bridge.post(notice("overflow", &session)),
        Err(NoticePostError::QueueFull {
            capacity: NOTICE_QUEUE_CAPACITY,
        }),
        "pending TUI queue must still count against the shared budget"
    );

    let delivered = inbox.take_notices(&session);
    assert_eq!(delivered.len(), NOTICE_QUEUE_CAPACITY);
    assert_eq!(
        bridge.post(notice("still-full", &session)),
        Err(NoticePostError::QueueFull {
            capacity: NOTICE_QUEUE_CAPACITY,
        }),
        "taken-but-uncommitted notices remain undelivered"
    );

    inbox.commit_delivered_notices(delivered.len());
    bridge
        .post(notice("after-delivery", &session))
        .expect("delivery frees budget for a new notice");
}

fn notice(run_id: &str, parent_session_id: &SessionId) -> SubagentNotice {
    SubagentNotice {
        run_id: run_id.into(),
        agent_id: "worker".into(),
        parent_session_id: parent_session_id.clone(),
        message: "blocked on schema".into(),
    }
}
