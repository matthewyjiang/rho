use rho_sdk::{model::Message, Error};

use super::InteractiveRuntime;

pub(super) struct CompactTaskResult {
    outcome: Result<rho_sdk::CompactionOutcome, Error>,
}

pub(crate) enum CompactTaskPoll {
    Finished(anyhow::Result<Option<rho_sdk::CompactionOutcome>>),
    Cancelled,
}

/// Session-owned compact work that can run off the TUI input loop.
struct CompactTask {
    session: rho_sdk::Session,
}

impl CompactTask {
    async fn run(self) -> CompactTaskResult {
        CompactTaskResult {
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

    /// Pre-prompt auto-compaction intentionally uses the same task and
    /// half-current retention cap as `/compact`. SDK in-run automatic requests
    /// use the configured window target without that manual cap.
    pub(crate) fn should_auto_compact(&self) -> bool {
        use crate::diagnostics::{CompactionContext, IdleCompactionCheck, IdleCompactionReason};

        let estimate = self.sessions.session().context_estimate();
        let context = CompactionContext::new(estimate, self.context_window, &self.compaction);
        let reason = if !self.compaction.auto_compact {
            IdleCompactionReason::Disabled
        } else if self.is_session_busy() {
            IdleCompactionReason::SessionBusy
        } else if let Some(threshold) = context.threshold_tokens {
            if estimate.tokens() < threshold {
                IdleCompactionReason::BelowThreshold
            } else if !self.can_compact() {
                IdleCompactionReason::NoCompactableHistory
            } else {
                IdleCompactionReason::Ready
            }
        } else {
            IdleCompactionReason::UnknownContextWindow
        };
        self.record_context_estimate(estimate);
        tracing::debug!(?context, ?reason, "idle automatic compaction check");
        self.diagnostics
            .record_idle_compaction(IdleCompactionCheck { context, reason });
        reason == IdleCompactionReason::Ready
    }

    /// Mirrors the manual-trigger partition `ModelCompactor` falls back to, so
    /// the TUI only offers `/compact` when it would remove something.
    pub(crate) fn can_compact_messages(&self, messages: &[Message]) -> bool {
        let tools = self.tools.specs();
        let target_tokens = self.compaction.target_tokens_for_context(
            self.context_window,
            rho_sdk::CompactionTrigger::Manual,
            self.sessions.session().estimate_context(messages),
        );
        crate::compaction::partition_messages_for_compaction(messages, &tools, target_tokens)
            .is_some()
    }

    pub(crate) fn begin_compact_task(&mut self) -> anyhow::Result<()> {
        if self.is_session_busy() {
            anyhow::bail!("session is busy");
        }
        let task = CompactTask {
            session: self.sessions.session().clone(),
        };
        self.pending_compact = Some(tokio::spawn(task.run()));
        Ok(())
    }

    pub(crate) async fn poll_compact_task(&mut self) -> Option<CompactTaskPoll> {
        let handle = self.pending_compact.as_mut()?;
        if !handle.is_finished() {
            return None;
        }
        let handle = self.pending_compact.take()?;
        Some(self.join_compact_task(handle).await)
    }

    /// Abort an in-flight job. If the join handle already finished, persist that
    /// result instead of discarding a committed compact.
    pub(crate) async fn abort_compact_task(&mut self) -> Option<CompactTaskPoll> {
        let handle = self.pending_compact.take()?;
        if !handle.is_finished() {
            handle.abort();
        }
        Some(self.join_compact_task(handle).await)
    }

    async fn join_compact_task(
        &mut self,
        handle: tokio::task::JoinHandle<CompactTaskResult>,
    ) -> CompactTaskPoll {
        match handle.await {
            Ok(result) => CompactTaskPoll::Finished(self.complete_compact_task(result).await),
            Err(error) if error.is_cancelled() => CompactTaskPoll::Cancelled,
            Err(error) => CompactTaskPoll::Finished(Err(anyhow::anyhow!(error))),
        }
    }

    /// Inline compact for tests. Does not pin a busy flag across `.await`, so
    /// dropping the future leaves the runtime idle.
    #[cfg(test)]
    pub(crate) async fn compact(&mut self) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        if self.is_session_busy() {
            anyhow::bail!("session is busy");
        }
        let outcome = self.sessions.session().compact().await?;
        self.apply_compact_outcome(outcome).await
    }

    async fn complete_compact_task(
        &mut self,
        result: CompactTaskResult,
    ) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        let outcome = result.outcome?;
        self.apply_compact_outcome(outcome).await
    }

    async fn apply_compact_outcome(
        &mut self,
        outcome: rho_sdk::CompactionOutcome,
    ) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
        if let Err(error) = self.sessions.save_compaction_snapshot(&[], &outcome) {
            // Compact mutates live history first. A failed save truncates the
            // partial append, so capturing after failure still reads the
            // previous complete leaf without paying a parse on success.
            let (checkpoint, capture_error) = match self.capture_durable_session() {
                Ok(checkpoint) => (checkpoint, None),
                Err(capture_error) => (None, Some(capture_error)),
            };
            let rollback = self.restore_durable_session(checkpoint).await;
            return match (capture_error, rollback) {
                (None, Ok(())) => Err(error),
                (Some(capture_error), Ok(())) => Err(anyhow::anyhow!(
                    "{error}; could not capture rollback checkpoint: {capture_error}"
                )),
                (None, Err(rollback_error)) => Err(anyhow::anyhow!(
                    "{error}; could not restore durable state: {rollback_error}"
                )),
                (Some(capture_error), Err(rollback_error)) => Err(anyhow::anyhow!(
                    "{error}; could not capture rollback checkpoint: {capture_error}; could not restore durable state: {rollback_error}"
                )),
            };
        }
        self.refresh_context_usage();
        if crate::compaction::outcome_reduced_context(&outcome) {
            self.invalidate_live_context();
            Ok(Some(outcome))
        } else {
            Ok(None)
        }
    }
}
