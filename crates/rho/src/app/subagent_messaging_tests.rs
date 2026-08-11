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

    let (mut receiver, permits) = bridge.bind_parent();
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
    // Reading from the transport alone must not free budget: delivery/discards do.
    assert_eq!(permits.outstanding(), NOTICE_QUEUE_CAPACITY);
    permits.release(NOTICE_QUEUE_CAPACITY);
    bridge
        .post(sample_notice("after-release"))
        .expect("release frees a slot");
}

// Covers: a stale inbox release after rebind must not free the new generation's
// budget or panic. Old and new generations own independent counters.
// Owner: app notice bridge
#[test]
fn notice_permit_release_is_scoped_to_binding_generation() {
    let bridge = SubagentNoticeBridge::new();
    let (_old_receiver, old_permits) = bridge.bind_parent();
    bridge
        .post(sample_notice("old"))
        .expect("first generation accepts a notice");
    assert_eq!(old_permits.outstanding(), 1);

    let (_new_receiver, new_permits) = bridge.bind_parent();
    assert_eq!(new_permits.outstanding(), 0);
    bridge
        .post(sample_notice("new"))
        .expect("replacement generation starts with a fresh budget");
    assert_eq!(new_permits.outstanding(), 1);

    // Late discard from the abandoned binding only touches its own counter.
    old_permits.release(1);
    assert_eq!(old_permits.outstanding(), 0);
    assert_eq!(new_permits.outstanding(), 1);
    assert_eq!(
        bridge.post(sample_notice("still-counts")),
        Ok(()),
        "active generation still tracks its own accepted notice"
    );
    assert_eq!(new_permits.outstanding(), 2);
}

// Covers: posts that reserved against a binding which is then replaced fail
// closed when the old receiver is dropped, without corrupting the new budget.
// Owner: app notice bridge
#[test]
fn notice_post_after_rebind_uses_only_the_active_generation() {
    let bridge = SubagentNoticeBridge::new();
    let (old_receiver, old_permits) = bridge.bind_parent();
    drop(old_receiver);

    let (mut new_receiver, new_permits) = bridge.bind_parent();
    bridge
        .post(sample_notice("active"))
        .expect("active binding accepts posts");
    assert_eq!(new_receiver.try_recv().unwrap().run_id, "active");
    assert_eq!(new_permits.outstanding(), 1);
    assert_eq!(old_permits.outstanding(), 0);

    // Fill the active generation; the abandoned counter must not absorb load.
    for index in 1..NOTICE_QUEUE_CAPACITY {
        bridge
            .post(sample_notice(&format!("n{index}")))
            .expect("active generation fills to capacity");
    }
    assert_eq!(
        bridge.post(sample_notice("overflow")),
        Err(NoticePostError::QueueFull {
            capacity: NOTICE_QUEUE_CAPACITY,
        })
    );
    assert_eq!(new_permits.outstanding(), NOTICE_QUEUE_CAPACITY);
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
