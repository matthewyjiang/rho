use rho_sdk::{model::Message, Error};

use crate::session::Session as StoredSession;

use super::InteractiveRuntime;

impl InteractiveRuntime {
    pub(crate) fn is_compacting(&self) -> bool {
        self.compacting
    }

    pub(crate) fn is_session_busy(&self) -> bool {
        self.runs.is_active() || self.compacting
    }

    pub(crate) fn can_compact(&self) -> bool {
        self.can_compact_messages(&self.sessions.history())
    }

    /// Auto-compact should run the same compact task as `/compact`.
    pub(crate) fn should_auto_compact(&self) -> bool {
        let Some(window) = self.context_window else {
            return false;
        };
        let Some(threshold) = self.compaction.threshold_tokens(window) else {
            return false;
        };
        if !self.can_compact() {
            return false;
        }
        let tokens = rho_sdk::model::context::estimate_context_tokens(
            &self.sessions.history(),
            &self.tools.specs(),
        );
        tokens >= threshold
    }

    pub(crate) fn can_compact_messages(&self, messages: &[Message]) -> bool {
        let target_tokens = self
            .context_window
            .map(|window| self.compaction.target_tokens(window))
            .unwrap_or(u64::MAX / 2);
        crate::compaction::partition_messages_for_compaction(
            messages,
            &self.tools.specs(),
            target_tokens,
        )
        .is_some()
    }

    pub(crate) fn begin_compact_task(&mut self) -> anyhow::Result<CompactTask> {
        if self.is_session_busy() {
            anyhow::bail!("session is busy");
        }
        self.compacting = true;
        Ok(CompactTask {
            session: self.sessions.session().clone(),
            checkpoint: self.capture_durable_session()?,
        })
    }

    pub(crate) fn abort_compact_task(&mut self) {
        self.compacting = false;
    }

    pub(crate) async fn complete_compact_task(
        &mut self,
        result: CompactTaskResult,
    ) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        self.compacting = false;
        let outcome = result.outcome?;
        if let Err(error) = self.sessions.save_compaction_snapshot(&[], &outcome) {
            let rollback = self.restore_durable_session(result.checkpoint).await;
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "{error}; could not restore durable state: {rollback_error}"
                )),
            };
        }
        let reduced = outcome.current_messages() < outcome.previous_messages()
            || outcome.removed_tokens() > 0;
        if reduced {
            self.runs.note_manual_compaction(self.context_window);
            self.invalidate_live_context();
            Ok(Some(outcome))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn compact(&mut self) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        let task = self.begin_compact_task()?;
        let result = task.run().await;
        self.complete_compact_task(result).await
    }
}

/// Session-owned compact work that can run off the TUI input loop.
pub(crate) struct CompactTask {
    session: rho_sdk::Session,
    checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
}

pub(crate) struct CompactTaskResult {
    checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
    outcome: Result<rho_sdk::CompactionOutcome, Error>,
}

impl CompactTask {
    pub(crate) async fn run(self) -> CompactTaskResult {
        CompactTaskResult {
            checkpoint: self.checkpoint,
            outcome: self.session.compact().await,
        }
    }
}
