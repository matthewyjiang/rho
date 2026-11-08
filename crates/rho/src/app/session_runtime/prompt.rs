//! A prompt's lifecycle. A host starts each prompt with
//! [`SessionRuntime::begin_prompt`] and holds the returned [`PromptScope`]
//! until the prompt ends. Every in-prompt call (a turn, `/compact`,
//! `/workflow:`, an `/agent:` rebind, the plan handoff) takes the scope, so
//! the type, not a convention, keeps them within a prompt. Everything else a
//! prompt's end needs is the runtime's own: a turn notes a credential
//! failure for the next prompt to renew, and a turn that reached the model's
//! answer asks to name the session (see `title.rs`).

use rho_sdk::CancellationToken;

use super::{
    prompt_gate::{ActiveWork, PromptGeneration},
    SessionHandle, SessionRuntime,
};
use crate::config::Config;

/// One prompt the runtime started, held for the whole prompt. Dropping it
/// ends the prompt, however the prompt returns, so a cancel aimed at it does
/// not reach the next one.
///
/// Each scope belongs to one prompt generation: a scope dropped only after
/// a newer prompt began never ends that prompt, and reads its own as
/// cancelled.
#[must_use = "dropping a PromptScope ends the prompt at once"]
pub(crate) struct PromptScope {
    cancellation: CancellationToken,
    handle: SessionHandle,
    generation: PromptGeneration,
}

impl PromptScope {
    /// Opens the next prompt on `handle`'s gate.
    pub(super) fn begin(handle: &SessionHandle) -> Self {
        let (cancellation, generation) = handle.gate().begin();
        Self {
            cancellation,
            handle: handle.clone(),
            generation,
        }
    }

    /// Trips when the prompt is cancelled before its first run starts, and
    /// is already cancelled when the session is closing, so work the host
    /// does first, such as expanding a command, can stop.
    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Whether the prompt stopped only because its session goes on
    /// elsewhere: another host took it over, the connection is ending, or a
    /// newer prompt began. Such a prompt keeps state a later prompt resumes,
    /// which a cancel or a retiring close drops.
    pub(crate) fn continues_elsewhere(&self) -> bool {
        self.handle.gate().continues_elsewhere(self.generation)
    }

    /// Whether this prompt was cancelled, at any point so far, or the
    /// session is closing.
    pub(super) fn is_cancelled(&self) -> bool {
        self.handle.gate().is_cancelled(self.generation)
    }

    /// Makes `work` what this prompt runs now, so a cancel reaches it, and
    /// one that already landed cancels it at once; a model run's steering
    /// goes to [`SessionHandle::steer`].
    pub(super) fn activate(&self, work: ActiveWork) {
        self.handle.gate().activate(self.generation, work);
    }
}

impl Drop for PromptScope {
    fn drop(&mut self) {
        self.handle.gate().finish(self.generation);
    }
}

impl SessionRuntime {
    /// Starts a prompt: refuses it while a compaction's rewrite is unsaved,
    /// opens the window in which a cancel reaches it, and renews credentials
    /// a logout or a refused request left stale. Hold the returned scope
    /// for the whole prompt and pass it to each of the prompt's calls.
    pub(crate) async fn begin_prompt(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<PromptScope> {
        if self.unsaved_compaction {
            return Err(super::UnsavedCompaction.into());
        }
        let scope = PromptScope::begin(&self.handle);
        self.renew_stale_credentials(process_config).await?;
        Ok(scope)
    }
}
