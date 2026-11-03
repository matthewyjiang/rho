//! Forwards polls to a stream only after that stream has woken its task.
//!
//! crossterm's `EventStream` is costly to poll spuriously. While no input is
//! pending, its wake thread blocks in `poll` holding crossterm's global reader
//! lock, and each `poll_next` spins on that lock (`sched_yield`) before it
//! returns `Pending`. `tokio::select!` polls every branch whenever any branch
//! wakes, so a streamed reply made the UI thread pay that spin once per model
//! event. Behind this gate, the terminal stream is polled once per real wake.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
};

use futures_util::{task::AtomicWaker, Stream, StreamExt};

pub(super) struct WakeGated<S> {
    stream: S,
    gate: Arc<Gate>,
}

/// Shared between the gated stream and the waker it hands the inner stream.
struct Gate {
    /// Set when the inner stream wakes, or after it yields an item (more may
    /// already be buffered). Starts set so the first poll reaches the stream.
    woken: AtomicBool,
    /// The latest outer task to poll, woken when the inner stream wakes.
    outer: AtomicWaker,
}

impl Wake for Gate {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
        self.outer.wake();
    }
}

impl<S: Stream + Unpin> WakeGated<S> {
    pub(super) fn new(stream: S) -> Self {
        Self {
            stream,
            gate: Arc::new(Gate {
                woken: AtomicBool::new(true),
                outer: AtomicWaker::new(),
            }),
        }
    }

    pub(super) async fn next(&mut self) -> Option<S::Item> {
        std::future::poll_fn(|context| self.poll_next(context)).await
    }

    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<S::Item>> {
        // Register before consuming the flag so a wake that lands in between
        // still reaches this task.
        self.gate.outer.register(context.waker());
        if !self.gate.woken.swap(false, Ordering::AcqRel) {
            return Poll::Pending;
        }
        let waker = Waker::from(Arc::clone(&self.gate));
        let item = self
            .stream
            .poll_next_unpin(&mut Context::from_waker(&waker));
        if item.is_ready() {
            self.gate.woken.store(true, Ordering::Release);
        }
        item
    }
}

#[cfg(test)]
#[path = "wake_gated_stream_tests.rs"]
mod tests;
