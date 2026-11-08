//! `/compact`: compact history between turns and save the result, for the
//! host to report however it shows compactions (ACP, for one, as a
//! compaction card).

use rho_sdk::{CancellationToken, CompactionOutcome};

use super::{prompt_gate::ActiveWork, PromptScope, SessionRuntime};

impl SessionRuntime {
    /// Compacts within `prompt`, with optional `instructions` on what to
    /// keep, and saves the result; `None` when it was cancelled first. A
    /// failure on the provider's credentials has the next prompt renew them.
    pub(crate) async fn compact(
        &mut self,
        prompt: &PromptScope,
        instructions: Option<String>,
    ) -> anyhow::Result<Option<CompactionOutcome>> {
        let result = self.compact_and_save(prompt, instructions).await;
        self.note_credential_failure(&result);
        result
    }

    /// The SDK commits replacement history before `compact` returns, so the
    /// work runs as a task: a cancel aborts it only while it is still running.
    /// A task that already finished has committed, and its outcome is saved
    /// like any other, as the TUI's abort does.
    async fn compact_and_save(
        &mut self,
        prompt: &PromptScope,
        instructions: Option<String>,
    ) -> anyhow::Result<Option<CompactionOutcome>> {
        let cancel = CancellationToken::new();
        prompt.activate(ActiveWork::command(cancel.clone()));
        let session = self.built.session.clone();
        let mut task = tokio::spawn(async move {
            match instructions {
                Some(instructions) => session.compact_with_instructions(instructions).await,
                None => session.compact().await,
            }
        });
        let joined = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                task.abort();
                (&mut task).await
            }
            joined = &mut task => joined,
        };
        let outcome = match joined {
            // Keep the typed error in the chain, so a refused credential
            // still asks the host to sign in.
            Ok(outcome) => outcome.map_err(|error| {
                let message = format!("could not compact context: {error}");
                anyhow::Error::new(error).context(message)
            })?,
            Err(error) if error.is_cancelled() => return Ok(None),
            Err(error) => return Err(anyhow::anyhow!("could not compact context: {error}")),
        };
        let snapshot = self.durable_snapshot();
        // An ephemeral session has nothing to save.
        let saved = self.stored().and_then(|stored| {
            stored.map_or(Ok(()), |stored| {
                stored.save_compaction_snapshot(&snapshot, &[], &outcome)
            })
        });
        if let Err(error) = saved {
            // Live history, compaction accounting, and metadata already
            // moved on; refuse later prompts rather than run against
            // state a reload would not see.
            self.unsaved_compaction = true;
            return Err(error.context("could not save compacted context"));
        }
        Ok(Some(outcome))
    }
}
