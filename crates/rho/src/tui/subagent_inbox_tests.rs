use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::SubagentInbox;
use crate::app::subagent_messaging::SubagentNotice;

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

fn notice(run_id: &str, parent_session_id: &SessionId) -> SubagentNotice {
    SubagentNotice {
        run_id: run_id.into(),
        agent_id: "worker".into(),
        parent_session_id: parent_session_id.clone(),
        message: "blocked on schema".into(),
    }
}
