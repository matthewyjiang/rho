//! The gate that carries a session's prompt cancellation.

use std::sync::Mutex;

use rho_sdk::CancellationToken;

/// Carries cancellation for the session's current prompt. A runtime runs one
/// prompt at a time, since its host holds it exclusively for each (ACP, for
/// one, takes its session slot with `try_lock`), so this gate only has to
/// cover the window between prompt entry and the run's token existing.
/// While `Starting`, a cancel trips the prompt token that `begin` returned,
/// so command expansion can stop, and is replayed onto the run's token as
/// soon as `activate` receives it. `cancel` is a no-op while idle.
///
/// Each `begin` opens a new [`PromptGeneration`], and `activate`, `finish`,
/// and `is_cancelled` act for one generation only, so a prompt's scope that
/// outlives it (dropped after the next prompt began) never finishes or
/// arms the newer prompt; to it, its own prompt reads as cancelled.
///
/// `close` is terminal: it cancels the current prompt, and every prompt that
/// begins later starts cancelled, so a prompt that took the host just before
/// teardown cannot run a turn the teardown would wait on.
///
/// `suspend` and `close(Closing::Disconnected)` also tell the prompt its
/// session goes on elsewhere (see [`PromptGate::continues_elsewhere`]).
pub(super) struct PromptGate {
    slot: Mutex<Slot>,
}

struct Slot {
    state: PromptGateState,
    /// The current prompt was suspended rather than cancelled.
    suspended: bool,
    /// The prompt `begin` opened last.
    generation: PromptGeneration,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting(CancellationToken),
    Active(CancellationToken),
    Closed(Closing),
}

/// Which begun prompt a scope belongs to: `begin` numbers each prompt one
/// past the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PromptGeneration(u64);

/// Why a session stops taking prompts for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Closing {
    /// The host closed or deleted the session (ACP `session/close` or
    /// `session/delete`): its work ends with it.
    Retired,
    /// The host's connection ends, so it may reopen the session in another
    /// process, as T3 Code does after it restarts.
    Disconnected,
}

impl PromptGate {
    pub(super) fn new() -> Self {
        Self {
            slot: Mutex::new(Slot {
                state: PromptGateState::Idle,
                suspended: false,
                generation: PromptGeneration(0),
            }),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Opens the starting window for a new prompt generation, dropping any
    /// cancel that arrived for the previous one. The returned token trips on
    /// a cancel that lands before `activate`, and is already cancelled once
    /// closed.
    pub(super) fn begin(&self) -> (CancellationToken, PromptGeneration) {
        let token = CancellationToken::new();
        let mut slot = self.slot();
        let generation = PromptGeneration(slot.generation.0.wrapping_add(1));
        slot.generation = generation;
        if matches!(slot.state, PromptGateState::Closed(_)) {
            token.cancel();
        } else {
            slot.state = PromptGateState::Starting(token.clone());
            slot.suspended = false;
        }
        (token, generation)
    }

    /// Makes `token` the cancellation of prompt `generation`'s current run.
    /// A prompt that is no longer current gets `token` cancelled instead.
    pub(super) fn activate(&self, generation: PromptGeneration, token: CancellationToken) {
        let mut slot = self.slot();
        let closed = matches!(slot.state, PromptGateState::Closed(_));
        let current = slot.generation == generation;
        // A prompt can run more than one turn (plan approval runs the
        // implementation turn), so a cancel that hit the previous turn's
        // token after it ended still cancels this one.
        if closed
            || !current
            || matches!(
                &slot.state,
                PromptGateState::Starting(previous) | PromptGateState::Active(previous)
                    if previous.is_cancelled()
            )
        {
            token.cancel();
        }
        if !closed && current {
            slot.state = PromptGateState::Active(token);
        }
    }

    /// Ends prompt `generation`; a no-op once a newer prompt began.
    pub(super) fn finish(&self, generation: PromptGeneration) {
        let mut slot = self.slot();
        if slot.generation == generation && !matches!(slot.state, PromptGateState::Closed(_)) {
            slot.state = PromptGateState::Idle;
        }
    }

    pub(super) fn cancel(&self) {
        match &self.slot().state {
            PromptGateState::Idle | PromptGateState::Closed(_) => {}
            PromptGateState::Starting(token) | PromptGateState::Active(token) => token.cancel(),
        }
    }

    /// Cancels the current prompt because another host takes the session
    /// over, as an ACP `session/load` of a live session does.
    pub(super) fn suspend(&self) {
        let mut slot = self.slot();
        if let PromptGateState::Starting(token) | PromptGateState::Active(token) = &slot.state {
            token.cancel();
            slot.suspended = true;
        }
    }

    /// Cancels the current prompt and every later one, for a session that
    /// is being torn down.
    pub(super) fn close(&self, closing: Closing) {
        let mut slot = self.slot();
        if let PromptGateState::Starting(token) | PromptGateState::Active(token) = &slot.state {
            token.cancel();
        }
        slot.state = PromptGateState::Closed(closing);
    }

    /// Whether prompt `generation` was cancelled, a newer prompt replaced
    /// it, or the session is closing.
    pub(super) fn is_cancelled(&self, generation: PromptGeneration) -> bool {
        let slot = self.slot();
        if slot.generation != generation {
            return true;
        }
        match &slot.state {
            PromptGateState::Idle => false,
            PromptGateState::Closed(_) => true,
            PromptGateState::Starting(token) | PromptGateState::Active(token) => {
                token.is_cancelled()
            }
        }
    }

    pub(super) fn is_closed(&self) -> bool {
        matches!(self.slot().state, PromptGateState::Closed(_))
    }

    /// Whether the current prompt stopped only because its session goes on
    /// elsewhere: another host took it over, or the connection is ending.
    /// Such a prompt keeps state a later prompt resumes, which a cancel or
    /// a retiring close drops.
    pub(super) fn continues_elsewhere(&self) -> bool {
        let slot = self.slot();
        slot.suspended || matches!(slot.state, PromptGateState::Closed(Closing::Disconnected))
    }

    /// Whether a new prompt may wait for the host instead of being refused:
    /// the current prompt was cancelled, none is running (the host is only
    /// held briefly by teardown or a setup request), or the session is
    /// closing, which ends the waiting prompt cancelled.
    pub(super) fn admits_queued_prompt(&self) -> bool {
        match &self.slot().state {
            PromptGateState::Idle | PromptGateState::Closed(_) => true,
            PromptGateState::Starting(token) | PromptGateState::Active(token) => {
                token.is_cancelled()
            }
        }
    }
}

#[cfg(test)]
#[path = "prompt_gate_tests.rs"]
mod tests;
