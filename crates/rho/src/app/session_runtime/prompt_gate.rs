//! The gate that carries a session's prompt cancellation and steering.

use std::{future::Future, sync::Mutex};

use rho_sdk::{
    model::{MessageTime, UserStamp},
    CancellationToken, SteeringHandle, SteeringId, SteeringRetraction, UserInput,
};

use super::persist::TurnCheckpoint;

/// Carries cancellation for the session's current prompt, and steering for
/// its model run. A runtime runs one prompt at a time, since its host holds
/// it exclusively for each, so this gate only has to cover the window
/// between prompt entry and the run's token existing.
/// While `Starting`, a cancel trips the prompt token that `begin` returned,
/// so command expansion can stop, and is replayed onto the run's token as
/// soon as `activate` receives it. `cancel` is a no-op while idle.
///
/// Steering reaches only a prompt's model run: `activate` installs it with
/// the run's token, and it goes when the prompt ends or a command such as
/// `/compact` takes its place.
///
/// Each `begin` opens a new [`PromptGeneration`] and cancels the previous
/// prompt's token, and `activate`, `finish`, and `is_cancelled` act for one
/// generation only, so a prompt's scope that outlives it (dropped after the
/// next prompt began) stops what it is waiting on and never finishes or arms
/// the newer prompt; to it, its own prompt reads as cancelled.
///
/// `close` is terminal: it cancels the current prompt, and every prompt that
/// begins later starts cancelled, so a prompt that took the host just before
/// teardown cannot run a turn the teardown would wait on.
pub(super) struct PromptGate {
    slot: Mutex<Slot>,
}

struct Slot {
    state: PromptGateState,
    /// The prompt `begin` opened last.
    generation: PromptGeneration,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting(CancellationToken),
    Active(ActiveWork),
    Closed,
}

impl PromptGateState {
    /// The current prompt's cancellation, while one is starting or running.
    fn current_token(&self) -> Option<&CancellationToken> {
        match self {
            Self::Starting(token) | Self::Active(ActiveWork { token, .. }) => Some(token),
            Self::Idle | Self::Closed => None,
        }
    }
}

/// The work a prompt runs right now, as a cancel or a steer reaches it.
#[derive(Debug)]
pub(super) struct ActiveWork {
    token: CancellationToken,
    /// Set for a model run; a command (`/compact`, `/workflow:`) takes no
    /// steering.
    steering: Option<RunSteering>,
}

/// A model run's steering, and the turn checkpoint that saves each steer's
/// shown row in place of its stamped model copy.
#[derive(Debug)]
struct RunSteering {
    handle: SteeringHandle,
    checkpoint: TurnCheckpoint,
}

impl ActiveWork {
    /// An SDK model run, which a steer reaches.
    pub(super) fn model_run(run: &rho_sdk::Run, checkpoint: TurnCheckpoint) -> Self {
        Self {
            token: run.cancellation_handle(),
            steering: Some(RunSteering {
                handle: run.steering_handle(),
                checkpoint,
            }),
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

impl PromptGate {
    pub(super) fn new() -> Self {
        Self {
            slot: Mutex::new(Slot {
                state: PromptGateState::Idle,
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
        if matches!(slot.state, PromptGateState::Closed) {
            token.cancel();
        } else {
            slot.state = PromptGateState::Starting(token.clone());
        }
        (token, generation)
    }

    /// Makes `work` what a cancel, and for a model run a steer, of prompt
    /// `generation` reaches. A prompt that is no longer current gets
    /// `work` cancelled instead.
    pub(super) fn activate(&self, generation: PromptGeneration, work: ActiveWork) {
        let mut slot = self.slot();
        let closed = matches!(slot.state, PromptGateState::Closed);
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
        if slot.generation == generation && !matches!(slot.state, PromptGateState::Closed) {
            slot.state = PromptGateState::Idle;
        }
    }

    pub(super) fn cancel(&self) {
        if let Some(token) = self.slot().state.current_token() {
            token.cancel();
        }
    }

    /// Stages `input` into the current prompt's model run, under the same
    /// lock a cancel takes, so a cancel either comes first and refuses the
    /// steer or comes after the run accepted it. `None` while no prompt
    /// runs, it is still starting, it runs a command, or it was cancelled:
    /// a cancelling run could accept a steer it never applies.
    ///
    /// The run gets `input` stamped with the time it arrived; the saved
    /// transcript shows it as sent.
    pub(super) fn steer(
        &self,
        input: UserInput,
    ) -> Option<
        Result<
            impl Future<Output = Result<SteeringId, rho_sdk::Error>> + Send + 'static,
            rho_sdk::Error,
        >,
    > {
        match &self.slot().state {
            PromptGateState::Active(work) if !work.token.is_cancelled() => {
                work.steering.as_ref().map(|steering| {
                    // The reply this steer interrupts is still running, so
                    // the stamp carries only when the steer arrived.
                    let model = input.clone().stamped(UserStamp {
                        sent: MessageTime::now(),
                        previous_reply: None,
                    });
                    steering.checkpoint.record_steer(&model, &input);
                    steering.handle.request_steer_retractable(model)
                })
            }
            PromptGateState::Active(_)
            | PromptGateState::Idle
            | PromptGateState::Starting(_)
            | PromptGateState::Closed => None,
        }
    }

    /// Asks the current prompt's model run to withdraw steer `id`, under
    /// the lock a steer takes. `None` exactly when [`Self::steer`] would be:
    /// a run that is not taking input has nothing staged to withdraw.
    pub(super) fn retract(
        &self,
        id: SteeringId,
    ) -> Option<
        Result<
            impl Future<Output = Result<SteeringRetraction, rho_sdk::Error>> + Send + 'static,
            rho_sdk::Error,
        >,
    > {
        match &self.slot().state {
            PromptGateState::Active(work) if !work.token.is_cancelled() => work
                .steering
                .as_ref()
                .map(|steering| steering.handle.request_steering_retraction(id)),
            PromptGateState::Active(_)
            | PromptGateState::Idle
            | PromptGateState::Starting(_)
            | PromptGateState::Closed => None,
        }
    }

    /// Cancels the current prompt and every later one, for a session that
    /// is being torn down.
    pub(super) fn close(&self) {
        let mut slot = self.slot();
        if let Some(token) = slot.state.current_token() {
            token.cancel();
        }
        slot.state = PromptGateState::Closed;
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
            PromptGateState::Closed => true,
            PromptGateState::Starting(token)
            | PromptGateState::Active(ActiveWork { token, .. }) => token.is_cancelled(),
        }
    }
}

#[cfg(test)]
#[path = "prompt_gate_tests.rs"]
mod tests;
