//! The handle a host shares with whoever acts on a session without holding
//! its runtime: cancelling or steering its prompt, or closing it.

use std::{future::Future, sync::Arc};

use rho_sdk::{SteeringId, UserInput};

use super::prompt_gate::PromptGate;

/// Why [`SessionHandle::steer`] staged nothing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SteerError {
    /// No model run takes input: no prompt runs, it is still starting or
    /// was cancelled, its run already ended, or it runs a command such as
    /// `/compact`.
    #[error("session has no running model turn to steer")]
    NotRunning,
    /// The running model turn refused the input, as when its command queue
    /// is full.
    #[error(transparent)]
    Refused(rho_sdk::Error),
}

/// A cloneable handle on one runtime. A host keeps it beside the runtime,
/// as the daemon does for each loaded session, so a cancel, steer, or close
/// acts at once instead of waiting for the running prompt to give the
/// runtime back.
/// Clones are the same handle.
#[derive(Clone)]
pub(crate) struct SessionHandle(Arc<PromptGate>);

impl SessionHandle {
    pub(super) fn new() -> Self {
        Self(Arc::new(PromptGate::new()))
    }

    /// A handle on no runtime, for host tests that publish an empty slot.
    #[cfg(test)]
    pub(in crate::app) fn detached_for_tests() -> Self {
        Self::new()
    }

    pub(super) fn gate(&self) -> &PromptGate {
        &self.0
    }

    /// Cancels the running prompt; a no-op while none runs.
    pub(crate) fn cancel(&self) {
        self.0.cancel();
    }

    /// Stages `input` for the running prompt's next provider turn, as a
    /// delegated child's parent does. The receipt resolves to the id the
    /// run accepted it under, which its `SteeringApplied` event names, or
    /// fails if the run ends first. Never waits, so a caller may hold a lock
    /// across it.
    pub(crate) fn steer(
        &self,
        input: UserInput,
    ) -> Result<impl Future<Output = Result<SteeringId, rho_sdk::Error>> + Send + 'static, SteerError>
    {
        self.0
            .steer(input)
            .ok_or(SteerError::NotRunning)?
            .map_err(|error| match error {
                rho_sdk::Error::RunFinished => SteerError::NotRunning,
                error => SteerError::Refused(error),
            })
    }

    /// Cancels the running prompt and every later one, for a session that
    /// is being torn down.
    pub(crate) fn close(&self) {
        self.0.close();
    }
}

#[cfg(test)]
#[path = "handle_tests.rs"]
mod tests;
