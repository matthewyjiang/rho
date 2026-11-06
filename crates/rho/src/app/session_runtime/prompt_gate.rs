//! The gate that carries a session's prompt cancellation and steering.

use std::sync::Mutex;

use rho_sdk::{CancellationToken, SteeringHandle};

/// Carries cancellation for the session's current prompt, and steering for
/// its model run. A runtime runs one prompt at a time, since its host holds
/// it exclusively for each (ACP, for one, takes its session slot with
/// `try_lock`), so this gate only has to cover the window between prompt
/// entry and the run's token existing.
/// While `Starting`, a cancel trips the prompt token that `begin` returned,
/// so command expansion can stop, and is replayed onto the run's token as
/// soon as `activate` receives it. `cancel` is a no-op while idle.
///
/// Steering reaches only a prompt's model run: `activate` installs it with
/// the run's token, and it goes when the prompt ends or a command such as
/// `/compact` takes its place.
///
/// Each `begin` opens a new [`PromptGeneration`] and cancels the previous
/// prompt's token, and `activate`, `finish`, `is_cancelled`, and
/// `continues_elsewhere` act for one generation only, so a prompt's scope
/// that outlives it (dropped after the next prompt began) stops what it is
/// waiting on and never finishes or arms the newer prompt; to it, its own
/// prompt reads as cancelled and as going on in the newer one.
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
    Active(ActiveWork),
    Closed(Closing),
}

impl PromptGateState {
    /// The current prompt's cancellation, while one is starting or running.
    fn current_token(&self) -> Option<&CancellationToken> {
        match self {
            Self::Starting(token) | Self::Active(ActiveWork { token, .. }) => Some(token),
            Self::Idle | Self::Closed(_) => None,
        }
    }
}

/// The work a prompt runs right now, as a cancel or a steer reaches it.
#[derive(Debug)]
pub(super) struct ActiveWork {
    token: CancellationToken,
    /// Set for a model run; a command (`/compact`, `/workflow:`) takes no
    /// steering.
    steering: Option<SteeringHandle>,
}

impl ActiveWork {
    /// An SDK model run, which a steer reaches.
    pub(super) fn model_run(run: &rho_sdk::Run) -> Self {
        Self {
            token: run.cancellation_handle(),
            steering: Some(run.steering_handle()),
        }
    }

    /// A command that stops on `cancel` and takes no steering.
    pub(super) fn command(token: CancellationToken) -> Self {
        Self {
            token,
            steering: None,
        }
    }
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
    /// cancel that arrived for the previous one and stopping whatever that
    /// prompt still waits on. The returned token trips on a cancel that
    /// lands before `activate`, and is already cancelled once closed.
    pub(super) fn begin(&self) -> (CancellationToken, PromptGeneration) {
        let token = CancellationToken::new();
        let mut slot = self.slot();
        if let Some(previous) = slot.state.current_token() {
            previous.cancel();
        }
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

    /// Makes `work` what a cancel, and for a model run a steer, of prompt
    /// `generation` reaches. A prompt that is no longer current gets
    /// `work` cancelled instead.
    pub(super) fn activate(&self, generation: PromptGeneration, work: ActiveWork) {
        let mut slot = self.slot();
        let closed = matches!(slot.state, PromptGateState::Closed(_));
        let current = slot.generation == generation;
        // A prompt can run more than one turn (plan approval runs the
        // implementation turn), so a cancel that hit the previous turn's
        // token after it ended still cancels this one.
        if closed
            || !current
            || slot
                .state
                .current_token()
                .is_some_and(CancellationToken::is_cancelled)
        {
            work.token.cancel();
        }
        if !closed && current {
            slot.state = PromptGateState::Active(work);
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
        if let Some(token) = self.slot().state.current_token() {
            token.cancel();
        }
    }

    /// The steering of the current prompt's model run; `None` while no
    /// prompt runs, it is still starting, it runs a command, or it was
    /// cancelled: a cancelling run could accept a steer it never applies.
    pub(super) fn steering(&self) -> Option<SteeringHandle> {
        match &self.slot().state {
            PromptGateState::Active(work) if !work.token.is_cancelled() => work.steering.clone(),
            PromptGateState::Active(_)
            | PromptGateState::Idle
            | PromptGateState::Starting(_)
            | PromptGateState::Closed(_) => None,
        }
    }

    /// Cancels the current prompt because another host takes the session
    /// over, as an ACP `session/load` of a live session does.
    pub(super) fn suspend(&self) {
        let mut slot = self.slot();
        if let Some(token) = slot.state.current_token() {
            token.cancel();
            slot.suspended = true;
        }
    }

    /// Cancels the current prompt and every later one, for a session that
    /// is being torn down.
    pub(super) fn close(&self, closing: Closing) {
        let mut slot = self.slot();
        if let Some(token) = slot.state.current_token() {
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
            PromptGateState::Starting(token)
            | PromptGateState::Active(ActiveWork { token, .. }) => token.is_cancelled(),
        }
    }

    pub(super) fn is_closed(&self) -> bool {
        matches!(self.slot().state, PromptGateState::Closed(_))
    }

    /// Whether prompt `generation` stopped only because its session goes on
    /// elsewhere: another host took it over, the connection is ending, or a
    /// newer prompt began. Such a prompt keeps state a later prompt resumes,
    /// which a cancel or a retiring close drops.
    pub(super) fn continues_elsewhere(&self, generation: PromptGeneration) -> bool {
        let slot = self.slot();
        slot.generation != generation
            || slot.suspended
            || matches!(slot.state, PromptGateState::Closed(Closing::Disconnected))
    }

    /// Whether a new prompt may wait for the host instead of being refused:
    /// the current prompt was cancelled, none is running (the host is only
    /// held briefly by teardown or a setup request), or the session is
    /// closing, which ends the waiting prompt cancelled.
    pub(super) fn admits_queued_prompt(&self) -> bool {
        match &self.slot().state {
            PromptGateState::Idle | PromptGateState::Closed(_) => true,
            PromptGateState::Starting(token)
            | PromptGateState::Active(ActiveWork { token, .. }) => token.is_cancelled(),
        }
    }
}

#[cfg(test)]
#[path = "prompt_gate_tests.rs"]
mod tests;
