//! The handle a host shares with whoever acts on a session without holding
//! its runtime: cancelling its prompt, retiring it, or handing its file to a
//! successor.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::prompt_gate::{Closing, PromptGate};

/// A cloneable handle on one runtime. A host keeps it beside the runtime,
/// as ACP does in its session slot, so a cancel, close, or reload acts at
/// once instead of waiting for the running prompt to give the runtime back.
/// Clones are the same handle; [`Self::same_runtime`] tells one runtime's
/// handle from a successor's.
#[derive(Clone)]
pub(crate) struct SessionHandle(Arc<Shared>);

struct Shared {
    gate: PromptGate,
    /// Set once another runtime owns the session file, so this one no
    /// longer writes it.
    replaced: AtomicBool,
}

impl SessionHandle {
    pub(super) fn new() -> Self {
        Self(Arc::new(Shared {
            gate: PromptGate::new(),
            replaced: AtomicBool::new(false),
        }))
    }

    /// A handle on no runtime, for host tests that publish an empty slot.
    #[cfg(test)]
    pub(crate) fn detached_for_tests() -> Self {
        Self::new()
    }

    /// Starts a prompt as the runtime does, for host tests that hold a slot
    /// with no runtime behind it.
    #[cfg(test)]
    pub(crate) fn start_prompt_for_tests(&self) {
        self.0.gate.begin();
    }

    pub(super) fn gate(&self) -> &PromptGate {
        &self.0.gate
    }

    /// Cancels the running prompt; a no-op while none runs.
    pub(crate) fn cancel(&self) {
        self.0.gate.cancel();
    }

    /// Cancels the running prompt because another host takes the session
    /// over, as an ACP `session/load` of a live session does. The prompt
    /// keeps what a later prompt resumes.
    pub(crate) fn suspend(&self) {
        self.0.gate.suspend();
    }

    /// Cancels the running prompt and every later one, for a session that
    /// is being torn down.
    pub(crate) fn close(&self, closing: Closing) {
        self.0.gate.close(closing);
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.0.gate.is_closed()
    }

    /// Whether a new prompt may wait for the runtime instead of being
    /// refused: the current prompt was cancelled, none is running, or the
    /// session is closing, which ends the waiting prompt cancelled.
    pub(crate) fn admits_queued_prompt(&self) -> bool {
        self.0.gate.admits_queued_prompt()
    }

    /// Hands the session file to another runtime, so this one no longer
    /// writes it.
    pub(crate) fn mark_replaced(&self) {
        self.0.replaced.store(true, Ordering::Release);
    }

    /// Takes the session file back after the runtime that was to replace
    /// this one failed to open, so this one saves again.
    pub(crate) fn reclaim(&self) {
        self.0.replaced.store(false, Ordering::Release);
    }

    /// Whether another runtime owns the session file.
    pub(crate) fn is_replaced(&self) -> bool {
        self.0.replaced.load(Ordering::Acquire)
    }

    /// Whether `other` is a handle on the same runtime.
    pub(crate) fn same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
