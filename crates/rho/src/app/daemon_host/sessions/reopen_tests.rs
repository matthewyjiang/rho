use std::pin::pin;

use futures_util::FutureExt;
use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::ReopenLocks;

// Covers: reopens of one session take its lock in turn, even when the first
// holder lets go while another waits, so a third never reopens beside that
// waiter; another session's lock is free meanwhile; and the last holder
// removes the session's lock.
// Owner: daemon session reopening (unit).
#[tokio::test]
async fn reopens_of_a_session_take_turns_and_leave_no_lock_behind() {
    let locks = ReopenLocks::default();
    let (session, other_session) = (SessionId::new(), SessionId::new());

    let first = locks.lock(&session).await;
    let mut second = pin!(locks.lock(&session));
    let second_while_first_holds = second.as_mut().now_or_never().is_some();
    let other = locks.lock(&other_session).now_or_never();
    let other_while_first_holds = other.is_some();
    drop(other);
    drop(first);
    let second = second.await;
    let mut third = pin!(locks.lock(&session));
    let third_while_second_holds = third.as_mut().now_or_never().is_some();
    drop(second);
    drop(third.await);

    assert_eq!(
        (
            second_while_first_holds,
            other_while_first_holds,
            third_while_second_holds,
            locks.is_empty(),
        ),
        (false, true, false, true)
    );
}
