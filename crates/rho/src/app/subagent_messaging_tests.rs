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
// Keeps the retired receiver alive across rebind (inbox ordering) so a stale
// enqueue path would still be open if post released the lock too early.
// Owner: app notice bridge
#[test]
fn notice_post_after_rebind_uses_only_the_active_generation() {
    let bridge = SubagentNoticeBridge::new();
    let (_old_receiver, old_permits) = bridge.bind_parent();

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

// Covers: rebind drains the retired receiver under the binding lock so a post
// that already returned Ok is retained with its matching permit generation
// instead of being dropped with the old receiver.
// Owner: app notice bridge
#[test]
fn notice_rebind_retains_in_flight_channel_notices_on_retired_generation() {
    let bridge = SubagentNoticeBridge::new();
    let (receiver, old_permits) = bridge.bind_parent();
    bridge
        .post(sample_notice("queued-a"))
        .expect("first notice accepted");
    bridge
        .post(sample_notice("queued-b"))
        .expect("second notice accepted");
    assert_eq!(old_permits.outstanding(), 2);

    let rebind = bridge.rebind_parent(Some(receiver));
    assert_eq!(
        rebind
            .retained
            .iter()
            .map(|notice| notice.run_id.as_str())
            .collect::<Vec<_>>(),
        vec!["queued-a", "queued-b"]
    );
    let retired = rebind
        .retired_permits
        .expect("prior binding exposes retired permits");
    assert_eq!(retired.outstanding(), 2);
    assert_eq!(rebind.permits.outstanding(), 0);
    bridge
        .post(sample_notice("new-gen"))
        .expect("replacement generation accepts posts");
    assert_eq!(rebind.permits.outstanding(), 1);
    // Freeing retained notices only touches the retired generation.
    retired.release(2);
    assert_eq!(old_permits.outstanding(), 0);
    assert_eq!(rebind.permits.outstanding(), 1);
}

// Covers: reserve and enqueue stay under the binding lock so a concurrent
// rebind cannot open a gap where post acknowledges a notice that receiver
// replacement then discards. Explicit barriers park inside the
// reservation-to-enqueue window; under the old unlock-before-enqueue ordering
// the binding lock probe succeeds and this test fails.
// Owner: app notice bridge
#[test]
fn notice_post_holds_binding_lock_from_reserve_through_enqueue() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Barrier,
    };
    use std::thread;
    use std::time::{Duration, Instant};

    let bridge = SubagentNoticeBridge::new();
    let (mut receiver, permits) = bridge.bind_parent();

    let in_gap = Arc::new(AtomicBool::new(false));
    // Released by the main thread once lock/rebind probes finish so enqueue
    // can complete under the same binding lock acquisition.
    let leave_gap = Arc::new(Barrier::new(2));
    let post_bridge = bridge.clone();
    let post_in_gap = Arc::clone(&in_gap);
    let post_leave_gap = Arc::clone(&leave_gap);

    let poster = thread::spawn(move || {
        post_bridge.post_with_enqueue_gap(sample_notice("locked"), &|| {
            post_in_gap.store(true, Ordering::SeqCst);
            post_leave_gap.wait();
        })
    });

    let entered = Instant::now();
    while !in_gap.load(Ordering::SeqCst) {
        assert!(
            entered.elapsed() < Duration::from_secs(10),
            "post should enter the reserve→enqueue gap"
        );
        thread::sleep(Duration::from_millis(1));
    }

    // Under the old ordering the lock was released before enqueue, so a rebind
    // could install here while the retired receiver was still live.
    assert!(
        bridge.binding_lock_held(),
        "binding lock must stay held from reserve through enqueue"
    );

    let rebind_bridge = bridge.clone();
    let rebind_started = Arc::new(AtomicBool::new(false));
    let rebind_flag = Arc::clone(&rebind_started);
    let rebind = thread::spawn(move || {
        rebind_flag.store(true, Ordering::SeqCst);
        rebind_bridge.bind_parent()
    });

    let rebind_seen = Instant::now();
    while !rebind_started.load(Ordering::SeqCst) {
        assert!(
            rebind_seen.elapsed() < Duration::from_secs(10),
            "rebind thread should start"
        );
        thread::sleep(Duration::from_millis(1));
    }
    // Failure-bound probe: while post remains in the gap, replacement cannot
    // finish installing. If enqueue released the lock, bind_parent would
    // complete and this assertion would fire.
    let blocked_until = Instant::now() + Duration::from_millis(50);
    while Instant::now() < blocked_until {
        assert!(
            !rebind.is_finished(),
            "rebind must wait for post to finish enqueue before replacing the binding"
        );
        thread::yield_now();
    }

    leave_gap.wait();
    let post_result = poster.join().expect("post thread");
    let (mut new_receiver, new_permits) = rebind.join().expect("rebind thread");
    assert_eq!(post_result, Ok(()));
    assert_eq!(receiver.try_recv().unwrap().run_id, "locked");
    assert_eq!(permits.outstanding(), 1);
    // Acknowledged notice stayed on the receiver it targeted; the replacement
    // binding is empty and does not inherit the old generation's budget.
    assert!(new_receiver.try_recv().is_err());
    assert_eq!(new_permits.outstanding(), 0);
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
        acknowledged: Default::default(),
        run_id: run_id.into(),
        agent_id: "worker".into(),
        parent_session_id: SessionId::from_string("session-1").unwrap(),
        message: "blocked on schema".into(),
    }
}
