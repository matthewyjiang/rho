use std::{future::Future, pin::pin, sync::Arc, task::Poll};

use pretty_assertions::assert_eq;
use tokio::{sync::Mutex, task::coop};

use super::Queued;

/// More polls of `consume_budget` than a tokio task's budget (128 units in
/// tokio 1.54) takes to spend; a tripwire should the budget grow.
const BUDGET_BOUND: usize = 1024;

// Covers: a place joined while the mutex is held is in the queue once
// `join` returns, so it takes the mutex before a place joined after it. A
// tokio upgrade that queued a waiter later than its first poll would let
// the second place take it, and a change or a turn overtake one queued
// before it.
// Owner: daemon session runtime queue.
#[tokio::test]
async fn a_place_joined_first_takes_the_mutex_first() {
    let lock = Arc::new(Mutex::new(()));
    let holder = lock.try_lock().unwrap();
    let Queued::Waiting(mut first) = Queued::join(&lock) else {
        panic!("a held mutex was taken");
    };
    let Queued::Waiting(mut second) = Queued::join(&lock) else {
        panic!("a held mutex was taken");
    };

    drop(holder);

    // The second first: it must not get the mutex the holder let go.
    let second_took_it = futures_util::poll!(second.as_mut()).is_ready();
    let first_took_it = futures_util::poll!(first.as_mut()).is_ready();
    assert_eq!((second_took_it, first_took_it), (false, true));
}

// Covers: a task that spent its coop budget still takes a free mutex as it
// joins, rather than holding a place that `join` reports waiting, which
// would let the next joiner's poll take the mutex ahead of it.
// Owner: daemon session runtime queue.
#[tokio::test]
async fn a_free_mutex_is_held_at_once_though_the_budget_is_spent() {
    let lock = Arc::new(Mutex::new(()));

    let (spent, held) = std::future::poll_fn(|cx| {
        // Spent within this one poll: the task gets a new budget only once
        // it yields.
        for _ in 0..BUDGET_BOUND {
            if !coop::has_budget_remaining() {
                break;
            }
            let _ = pin!(coop::consume_budget()).poll(cx);
        }
        let spent = !coop::has_budget_remaining();
        Poll::Ready((spent, matches!(Queued::join(&lock), Queued::Held(_))))
    })
    .await;

    assert_eq!((spent, held), (true, true));
}
