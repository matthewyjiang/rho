use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::{
    MessageValidationError, NoticePostError, SubagentNotice, SubagentNoticeBridge,
    ValidatedMessage, MAX_MESSAGE_BYTES, NOTICE_QUEUE_CAPACITY,
};

// Covers: empty and oversized bodies fail with named budgets
// Owner: app messaging policy
#[test]
fn validated_message_rejects_empty_and_oversized() {
    assert_eq!(
        ValidatedMessage::parse("   "),
        Err(MessageValidationError::Empty)
    );
    let too_big = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert_eq!(
        ValidatedMessage::parse(&too_big),
        Err(MessageValidationError::TooLarge {
            bytes: MAX_MESSAGE_BYTES + 1,
            max_bytes: MAX_MESSAGE_BYTES,
        })
    );
    assert_eq!(
        ValidatedMessage::parse("  hello  ").unwrap().into_string(),
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

// Covers: the steering slot is closed before publish and after clear, so a
// parent message outside the live window cannot silently vanish.
// Owner: app messaging policy
#[test]
fn steering_slot_is_closed_outside_the_live_window() {
    let slot = super::SteeringSlot::new();
    assert!(slot.handle().is_none());
    slot.clear();
    assert!(slot.handle().is_none());
}

fn sample_notice(run_id: &str) -> SubagentNotice {
    SubagentNotice {
        run_id: run_id.into(),
        agent_id: "worker".into(),
        parent_session_id: SessionId::from_string("session-1").unwrap(),
        message: "blocked on schema".into(),
    }
}
