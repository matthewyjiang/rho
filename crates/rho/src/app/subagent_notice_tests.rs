use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::{
    validate_message_text, MessageValidationError, NoticePostError, SubagentNotice,
    SubagentNoticeBridge, MAX_NOTICE_BYTES, NOTICE_QUEUE_CAPACITY,
};

// Covers: empty and oversized bodies fail with named budgets
// Owner: app notice policy
#[test]
fn validate_message_text_rejects_empty_and_oversized() {
    assert_eq!(
        validate_message_text("   ", MAX_NOTICE_BYTES),
        Err(MessageValidationError::Empty)
    );
    let too_big = "x".repeat(MAX_NOTICE_BYTES + 1);
    assert_eq!(
        validate_message_text(&too_big, MAX_NOTICE_BYTES),
        Err(MessageValidationError::TooLarge {
            bytes: MAX_NOTICE_BYTES + 1,
            max_bytes: MAX_NOTICE_BYTES,
        })
    );
    assert_eq!(
        validate_message_text("  hello  ", MAX_NOTICE_BYTES).unwrap(),
        "hello"
    );
}

// Covers: unbound and full queue fail loud instead of dropping silently
// Owner: app notice bridge
#[test]
fn notice_bridge_fails_closed_when_unbound_or_full() {
    let bridge = SubagentNoticeBridge::new();
    let notice = sample_notice("a1");
    assert_eq!(bridge.post(notice.clone()), Err(NoticePostError::Unbound));

    let mut receiver = bridge.bind_parent();
    for index in 0..NOTICE_QUEUE_CAPACITY {
        bridge
            .post(sample_notice(&format!("n{index}")))
            .expect("queue should accept up to capacity");
    }
    assert_eq!(
        bridge.post(sample_notice("overflow")),
        Err(NoticePostError::QueueFull {
            capacity: NOTICE_QUEUE_CAPACITY,
        })
    );
    assert_eq!(receiver.try_recv().unwrap().run_id, "n0");
}

fn sample_notice(run_id: &str) -> SubagentNotice {
    SubagentNotice {
        run_id: run_id.into(),
        agent_id: "worker".into(),
        parent_session_id: SessionId::from_string("session-1").unwrap(),
        message: "blocked on schema".into(),
    }
}
