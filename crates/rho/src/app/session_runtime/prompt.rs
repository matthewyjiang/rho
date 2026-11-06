//! A prompt's lifecycle. A host starts each prompt with
//! [`SessionRuntime::begin_prompt`], runs the prompt's turn or command while
//! it holds the returned [`PromptStart`], and drops it when the prompt ends.
//! Everything else a prompt's end needs is the runtime's own: a turn notes a
//! credential failure for the next prompt to renew, and a turn that reached
//! the model's answer asks to name the session (see `title.rs`).

use std::path::Path;

use rho_sdk::CancellationToken;

use super::{SessionHandle, SessionRuntime};
use crate::config::Config;

/// A prompt the runtime started. Dropping it ends the prompt, however the
/// prompt returns, so a cancel aimed at it does not reach the next one.
#[must_use = "dropping a PromptStart ends the prompt at once"]
pub(crate) struct PromptStart {
    cancellation: CancellationToken,
    handle: SessionHandle,
}

impl PromptStart {
    /// Trips when the prompt is cancelled before its first run starts, and
    /// is already cancelled when the session is closing, so work the host
    /// does first, such as expanding a command, can stop.
    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Whether the prompt stopped only because its session goes on
    /// elsewhere: another host took it over, or the connection is ending.
    /// Such a prompt keeps state a later prompt resumes, which a cancel or
    /// a retiring close drops.
    pub(crate) fn hands_off(&self) -> bool {
        self.handle.gate().hands_off()
    }
}

impl Drop for PromptStart {
    fn drop(&mut self) {
        self.handle.gate().finish();
    }
}

impl SessionRuntime {
    /// Starts a prompt: refuses it while a compaction's rewrite is unsaved,
    /// opens the window in which a cancel reaches it, and renews credentials
    /// a logout or a refused request left stale. Run the prompt's turns and
    /// commands while holding the returned start.
    pub(crate) async fn begin_prompt(
        &mut self,
        process_config: &Config,
        config_path: &Path,
    ) -> anyhow::Result<PromptStart> {
        if self.unsaved_compaction {
            anyhow::bail!(
                "could not continue session: its compacted context was not saved; load the session again"
            );
        }
        let start = PromptStart {
            cancellation: self.handle.gate().begin(),
            handle: self.handle.clone(),
        };
        self.renew_stale_credentials(process_config, config_path)
            .await?;
        Ok(start)
    }
}
