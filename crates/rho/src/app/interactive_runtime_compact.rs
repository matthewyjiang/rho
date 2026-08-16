use rho_sdk::{model::Message, Error};

use crate::session::Session as StoredSession;

use super::InteractiveRuntime;

pub(super) struct CompactTaskResult {
    checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
    outcome: Result<rho_sdk::CompactionOutcome, Error>,
}

pub(crate) enum CompactTaskPoll {
    Finished(anyhow::Result<Option<rho_sdk::CompactionOutcome>>),
    Cancelled,
}

/// Session-owned compact work that can run off the TUI input loop.
struct CompactTask {
    session: rho_sdk::Session,
    checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
}

impl CompactTask {
    async fn run(self) -> CompactTaskResult {
        CompactTaskResult {
            checkpoint: self.checkpoint,
            outcome: self.session.compact().await,
        }
    }
}

impl InteractiveRuntime {
    pub(crate) fn is_compacting(&self) -> bool {
        self.pending_compact.is_some()
    }

    pub(crate) fn is_session_busy(&self) -> bool {
        self.runs.is_active() || self.is_compacting()
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

    pub(crate) fn begin_compact_task(&mut self) -> anyhow::Result<()> {
        if self.is_session_busy() {
            anyhow::bail!("session is busy");
        }
        let task = CompactTask {
            session: self.sessions.session().clone(),
            checkpoint: self.capture_durable_session()?,
        };
        self.pending_compact = Some(tokio::spawn(task.run()));
        Ok(())
    }

    pub(crate) async fn poll_compact_task(&mut self) -> Option<CompactTaskPoll> {
        let handle = self.pending_compact.as_mut()?;
        if !handle.is_finished() {
            return None;
        }
        self.take_compact_task(false).await
    }

    /// Abort an in-flight job. If the join handle already finished, persist that
    /// result instead of discarding a committed compact.
    pub(crate) async fn abort_compact_task(&mut self) -> Option<CompactTaskPoll> {
        self.take_compact_task(true).await
    }

    async fn take_compact_task(&mut self, abort_if_running: bool) -> Option<CompactTaskPoll> {
        let handle = self.pending_compact.take()?;
        if abort_if_running && !handle.is_finished() {
            handle.abort();
        }
        Some(match handle.await {
            Ok(result) => CompactTaskPoll::Finished(self.complete_compact_task(result).await),
            Err(error) if error.is_cancelled() => CompactTaskPoll::Cancelled,
            Err(error) => CompactTaskPoll::Finished(Err(anyhow::anyhow!(error))),
        })
    }

    /// Inline compact for tests. Does not pin a busy flag across `.await`, so
    /// dropping the future leaves the runtime idle.
    #[cfg(test)]
    pub(crate) async fn compact(&mut self) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        if self.is_session_busy() {
            anyhow::bail!("session is busy");
        }
        let checkpoint = self.capture_durable_session()?;
        let outcome = self.sessions.session().compact().await?;
        self.apply_compact_outcome(checkpoint, outcome).await
    }

    async fn complete_compact_task(
        &mut self,
        result: CompactTaskResult,
    ) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        let outcome = result.outcome?;
        self.apply_compact_outcome(result.checkpoint, outcome).await
    }

    async fn apply_compact_outcome(
        &mut self,
        checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
        outcome: rho_sdk::CompactionOutcome,
    ) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        if let Err(error) = self.sessions.save_compaction_snapshot(&[], &outcome) {
            let rollback = self.restore_durable_session(checkpoint).await;
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "{error}; could not restore durable state: {rollback_error}"
                )),
            };
        }
        if crate::compaction::outcome_reduced_context(&outcome) {
            self.runs.note_manual_compaction(self.context_window);
            self.invalidate_live_context();
            Ok(Some(outcome))
        } else {
            Ok(None)
        }
    }
}
