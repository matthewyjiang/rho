use std::{
    future::Future,
    sync::Arc,
    task::{Context, Waker},
};

use pretty_assertions::assert_eq;

use super::ExecutionArbiter;
use crate::tool::{ToolExecutionPolicy, ToolResource, ToolResourceAccess};

// Covers: conflicting acquires wait for permit drop, while compatible acquires proceed.
// Owner: SDK execution-arbiter admission and permit lifetime.
#[tokio::test]
async fn conflicting_acquires_wait_until_the_earlier_permit_drops() {
    let read = ToolExecutionPolicy::resource_aware([ToolResourceAccess::shared(
        ToolResource::session_state(),
    )]);
    let write = ToolExecutionPolicy::resource_aware([ToolResourceAccess::exclusive(
        ToolResource::session_state(),
    )]);
    for (first_policy, second_policy, second_waits) in [
        (
            ToolExecutionPolicy::Exclusive,
            ToolExecutionPolicy::Exclusive,
            true,
        ),
        (ToolExecutionPolicy::Exclusive, read.clone(), true),
        (write.clone(), write, true),
        (read.clone(), read, false),
    ] {
        let arbiter = Arc::new(ExecutionArbiter::default());
        let first = arbiter.acquire(&first_policy).await;
        let mut second = Box::pin(arbiter.acquire(&second_policy));
        let mut cx = Context::from_waker(Waker::noop());
        let admission = second.as_mut().poll(&mut cx);
        assert_eq!(admission.is_pending(), second_waits);
        drop(first);
        if second_waits {
            assert!(second.as_mut().poll(&mut cx).is_ready());
        }
    }
}

// Covers: dropping a queued exclusive barrier cannot strand compatible calls.
// Owner: SDK execution-arbiter queue lifetime.
#[tokio::test]
async fn cancelled_execution_waiter_releases_its_barrier() {
    let arbiter = Arc::new(ExecutionArbiter::default());
    let read = ToolExecutionPolicy::resource_aware([ToolResourceAccess::shared(
        ToolResource::session_state(),
    )]);
    let exclusive = ToolExecutionPolicy::Exclusive;
    let first = arbiter.acquire(&read).await;
    let mut barrier = Box::pin(arbiter.acquire(&exclusive));
    let mut later_read = Box::pin(arbiter.acquire(&read));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(barrier.as_mut().poll(&mut cx).is_pending());
    assert!(later_read.as_mut().poll(&mut cx).is_pending());
    drop(barrier);
    let later = later_read.as_mut().poll(&mut cx);
    assert!(later.is_ready());
    drop((first, later));
    let mut final_exclusive = Box::pin(arbiter.acquire(&exclusive));
    assert!(final_exclusive.as_mut().poll(&mut cx).is_ready());
}
