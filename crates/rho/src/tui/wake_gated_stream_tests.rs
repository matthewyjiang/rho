use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use futures_util::{task::noop_waker_ref, Stream};
use pretty_assertions::assert_eq;

use super::WakeGated;

/// Counts polls and keeps the waker it was last polled with, like a stream
/// whose readiness is signalled by a background thread.
#[derive(Default)]
struct WakingStream {
    items: VecDeque<u8>,
    polls: usize,
    waker: Option<Waker>,
}

impl Stream for WakingStream {
    type Item = u8;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<u8>> {
        self.polls += 1;
        match self.items.pop_front() {
            Some(item) => Poll::Ready(Some(item)),
            None => {
                self.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

// Covers: spurious outer polls (select! waking for other branches) must not
// reach the terminal stream, and a real wake must still deliver every buffered
// item. Owner: TUI terminal input polling.
#[test]
fn polls_inner_stream_only_after_it_wakes() {
    let mut gated = WakeGated::new(WakingStream::default());
    let mut context = Context::from_waker(noop_waker_ref());

    assert_eq!(gated.poll_next(&mut context), Poll::Pending);
    for _ in 0..5 {
        assert_eq!(gated.poll_next(&mut context), Poll::Pending);
    }
    assert_eq!(gated.stream.polls, 1);

    gated.stream.items.extend([1, 2]);
    gated.stream.waker.take().expect("inner waker").wake();
    assert_eq!(gated.poll_next(&mut context), Poll::Ready(Some(1)));
    assert_eq!(gated.poll_next(&mut context), Poll::Ready(Some(2)));
    assert_eq!(gated.poll_next(&mut context), Poll::Pending);
    assert_eq!(gated.poll_next(&mut context), Poll::Pending);
    assert_eq!(gated.stream.polls, 4);
}
