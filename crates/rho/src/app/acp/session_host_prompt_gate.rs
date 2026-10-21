//! The gate that carries a session's prompt cancellation.

use std::sync::{Arc, Mutex};

use rho_sdk::CancellationToken;

/// Carries cancellation for the session's current prompt. The agent already
/// serializes prompts with `try_lock` on the host slot, so this gate only has
/// to cover the window between prompt entry and the run's token existing.
/// While `Starting`, a cancel trips the prompt token that `begin` returned,
/// so command expansion can stop, and is replayed onto the run's token as
/// soon as `activate` receives it. `cancel` is a no-op while idle.
///
/// `close` is terminal: it cancels the current prompt, and every prompt that
/// begins later starts cancelled, so a prompt that took the host just before
/// teardown cannot run a turn the teardown would wait on.
pub(in crate::app::acp) struct PromptGate {
    slot: Mutex<PromptGateState>,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting(CancellationToken),
    Active(CancellationToken),
    Closed,
}

impl PromptGate {
    pub(in crate::app::acp) fn new() -> Self {
        Self {
            slot: Mutex::new(PromptGateState::Idle),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, PromptGateState> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Opens the starting window for a prompt, dropping any cancel that
    /// arrived for the previous one. The returned token trips on a cancel
    /// that lands before `activate`, and is already cancelled once closed.
    pub(in crate::app::acp) fn begin(&self) -> CancellationToken {
        let token = CancellationToken::new();
        let mut slot = self.slot();
        if matches!(&*slot, PromptGateState::Closed) {
            token.cancel();
        } else {
            *slot = PromptGateState::Starting(token.clone());
        }
        token
    }

    pub(super) fn activate(&self, token: CancellationToken) {
        let mut slot = self.slot();
        let closed = matches!(&*slot, PromptGateState::Closed);
        // A prompt can run more than one turn (plan approval runs the
        // implementation turn), so a cancel that hit the previous turn's
        // token after it ended still cancels this one.
        if closed
            || matches!(
                &*slot,
                PromptGateState::Starting(previous) | PromptGateState::Active(previous)
                    if previous.is_cancelled()
            )
        {
            token.cancel();
        }
        if !closed {
            *slot = PromptGateState::Active(token);
        }
    }

    pub(in crate::app::acp) fn finish(&self) {
        let mut slot = self.slot();
        if !matches!(&*slot, PromptGateState::Closed) {
            *slot = PromptGateState::Idle;
        }
    }

    pub(in crate::app::acp) fn cancel(&self) {
        match &mut *self.slot() {
            PromptGateState::Idle | PromptGateState::Closed => {}
            PromptGateState::Starting(token) | PromptGateState::Active(token) => token.cancel(),
        }
    }

    /// Cancels the current prompt and every later one, for a session that
    /// is being torn down.
    pub(in crate::app::acp) fn close(&self) {
        let mut slot = self.slot();
        if let PromptGateState::Starting(token) | PromptGateState::Active(token) = &*slot {
            token.cancel();
        }
        *slot = PromptGateState::Closed;
    }

    /// Whether the running prompt was cancelled, or the session is closing.
    pub(in crate::app::acp) fn is_cancelled(&self) -> bool {
        match &*self.slot() {
            PromptGateState::Idle => false,
            PromptGateState::Closed => true,
            PromptGateState::Starting(token) | PromptGateState::Active(token) => {
                token.is_cancelled()
            }
        }
    }

    pub(in crate::app::acp) fn is_closed(&self) -> bool {
        matches!(&*self.slot(), PromptGateState::Closed)
    }

    /// Whether a new prompt may wait for the host instead of being refused:
    /// the current prompt was cancelled, none is running (the host is only
    /// held briefly by teardown or a setup request), or the session is
    /// closing, which ends the waiting prompt cancelled.
    pub(in crate::app::acp) fn admits_queued_prompt(&self) -> bool {
        match &*self.slot() {
            PromptGateState::Idle | PromptGateState::Closed => true,
            PromptGateState::Starting(token) | PromptGateState::Active(token) => {
                token.is_cancelled()
            }
        }
    }
}

pub(super) struct PromptGuard(pub(super) Arc<PromptGate>);

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.0.finish();
    }
}
