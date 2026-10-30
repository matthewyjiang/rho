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
///
/// `suspend` and `close(Closing::Disconnected)` also tell the prompt its
/// session goes on elsewhere (see [`PromptGate::hands_off`]).
pub(in crate::app::acp) struct PromptGate {
    slot: Mutex<Slot>,
}

struct Slot {
    state: PromptGateState,
    /// The current prompt was suspended rather than cancelled.
    suspended: bool,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting(CancellationToken),
    Active(CancellationToken),
    Closed(Closing),
}

/// Why a session stops taking prompts for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::acp) enum Closing {
    /// `session/close` or `session/delete`: its work ends with it.
    Retired,
    /// The connection ends, so the host may reopen the session in another
    /// process, as T3 Code does after it restarts.
    Disconnected,
}

impl PromptGate {
    pub(in crate::app::acp) fn new() -> Self {
        Self {
            slot: Mutex::new(Slot {
                state: PromptGateState::Idle,
                suspended: false,
            }),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
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
        if matches!(slot.state, PromptGateState::Closed(_)) {
            token.cancel();
        } else {
            *slot = Slot {
                state: PromptGateState::Starting(token.clone()),
                suspended: false,
            };
        }
        token
    }

    pub(super) fn activate(&self, token: CancellationToken) {
        let mut slot = self.slot();
        let closed = matches!(slot.state, PromptGateState::Closed(_));
        // A prompt can run more than one turn (plan approval runs the
        // implementation turn), so a cancel that hit the previous turn's
        // token after it ended still cancels this one.
        if closed
            || matches!(
                &slot.state,
                PromptGateState::Starting(previous) | PromptGateState::Active(previous)
                    if previous.is_cancelled()
            )
        {
            token.cancel();
        }
        if !closed {
            slot.state = PromptGateState::Active(token);
        }
    }

    pub(in crate::app::acp) fn finish(&self) {
        let mut slot = self.slot();
        if !matches!(slot.state, PromptGateState::Closed(_)) {
            slot.state = PromptGateState::Idle;
        }
    }

    pub(in crate::app::acp) fn cancel(&self) {
        match &self.slot().state {
            PromptGateState::Idle | PromptGateState::Closed(_) => {}
            PromptGateState::Starting(token) | PromptGateState::Active(token) => token.cancel(),
        }
    }

    /// Cancels the current prompt because another host takes the session
    /// over, as `session/load` of a live session does.
    pub(in crate::app::acp) fn suspend(&self) {
        let mut slot = self.slot();
        if let PromptGateState::Starting(token) | PromptGateState::Active(token) = &slot.state {
            token.cancel();
            slot.suspended = true;
        }
    }

    /// Cancels the current prompt and every later one, for a session that
    /// is being torn down.
    pub(in crate::app::acp) fn close(&self, closing: Closing) {
        let mut slot = self.slot();
        if let PromptGateState::Starting(token) | PromptGateState::Active(token) = &slot.state {
            token.cancel();
        }
        slot.state = PromptGateState::Closed(closing);
    }

    /// Whether the running prompt was cancelled, or the session is closing.
    pub(in crate::app::acp) fn is_cancelled(&self) -> bool {
        match &self.slot().state {
            PromptGateState::Idle => false,
            PromptGateState::Closed(_) => true,
            PromptGateState::Starting(token) | PromptGateState::Active(token) => {
                token.is_cancelled()
            }
        }
    }

    pub(in crate::app::acp) fn is_closed(&self) -> bool {
        matches!(self.slot().state, PromptGateState::Closed(_))
    }

    /// Whether the current prompt stopped only because its session goes on
    /// elsewhere: another host took it over, or the connection is ending.
    /// Such a prompt keeps state a later prompt resumes, which a cancel or
    /// a retiring close drops.
    pub(in crate::app::acp) fn hands_off(&self) -> bool {
        let slot = self.slot();
        slot.suspended || matches!(slot.state, PromptGateState::Closed(Closing::Disconnected))
    }

    /// Whether a new prompt may wait for the host instead of being refused:
    /// the current prompt was cancelled, none is running (the host is only
    /// held briefly by teardown or a setup request), or the session is
    /// closing, which ends the waiting prompt cancelled.
    pub(in crate::app::acp) fn admits_queued_prompt(&self) -> bool {
        match &self.slot().state {
            PromptGateState::Idle | PromptGateState::Closed(_) => true,
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
