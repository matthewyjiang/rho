//! Main-loop compact job.
//!
//! `/compact` and pre-turn auto-compact start this job. In-run auto-compact
//! stays inside the SDK turn and applies the same card events from
//! `compaction_display` / `event_adapter`.

use crate::app::interactive_runtime::CompactTaskPoll;

use super::{
    compaction_display::CompactionUiOutcome,
    event_adapter::{compact_finished_event, compact_started_event},
    App, InteractiveModelSelection, InteractiveRuntime, ViewModelEvent,
};

/// Work to run after the current compact job settles.
#[derive(Default)]
pub(super) enum CompactFollowUp {
    #[default]
    None,
    ContextHandoff {
        target_selection: Option<InteractiveModelSelection>,
        had_source: bool,
    },
}

impl App {
    pub(super) fn start_compact(
        &mut self,
        agent: &mut InteractiveRuntime,
        follow_up: CompactFollowUp,
    ) -> anyhow::Result<()> {
        if agent.is_compacting() {
            if !matches!(follow_up, CompactFollowUp::None) {
                anyhow::bail!("already compacting context");
            }
            self.notify_status("already compacting context");
            return Ok(());
        }
        agent.begin_compact_task()?;
        self.pending.steering_prompts_mut().clear();
        self.pending_input_changed();
        self.set_status("compacting context");
        self.begin_compact_ui();
        self.turn.start_loading();
        self.apply_compact_view_event(compact_started_event());
        self.compact_follow_up = follow_up;
        Ok(())
    }

    /// User cancel (`esc`). Leaves queued follow-ups in the pending-input list
    /// and finishes a handoff follow-up as "not compacted".
    pub(super) async fn cancel_compact(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(poll) = agent.abort_compact_task().await else {
            return Ok(false);
        };
        self.settle_compact_poll(poll, agent).await?;
        Ok(true)
    }

    /// Drop the job and its follow-up. Used by `/new` and process exit.
    /// A finished result is still persisted so quit cannot lose a committed compact.
    pub(super) async fn abort_compact(&mut self, agent: &mut InteractiveRuntime) -> bool {
        let started =
            agent.is_compacting() || !matches!(self.compact_follow_up, CompactFollowUp::None);
        if let Some(CompactTaskPoll::Finished(result)) = agent.abort_compact_task().await {
            // Persist succeeded; do not apply handoff/turn follow-ups on teardown.
            let _ = std::mem::take(&mut self.compact_follow_up);
            self.finish_compact_ui(CompactionUiOutcome::from_task_result(result));
            return true;
        }
        self.compact_follow_up = CompactFollowUp::None;
        if started {
            self.finish_compact_ui(CompactionUiOutcome::Cancelled);
        }
        started
    }

    pub(super) async fn poll_compact(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(poll) = agent.poll_compact_task().await else {
            return Ok(false);
        };
        self.settle_compact_poll(poll, agent).await?;
        Ok(true)
    }

    async fn settle_compact_poll(
        &mut self,
        poll: CompactTaskPoll,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let outcome = match poll {
            CompactTaskPoll::Cancelled => CompactionUiOutcome::Cancelled,
            CompactTaskPoll::Finished(result) => CompactionUiOutcome::from_task_result(result),
        };
        let succeeded = matches!(outcome, CompactionUiOutcome::Completed(_));
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        if outcome.starts_follow_ups() {
            self.start_follow_ups = Some(false);
        }
        self.finish_compact_ui(outcome);
        let follow_up = std::mem::take(&mut self.compact_follow_up);
        self.apply_compact_follow_up(follow_up, succeeded, agent)
            .await
    }

    async fn apply_compact_follow_up(
        &mut self,
        follow_up: CompactFollowUp,
        succeeded: bool,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match follow_up {
            CompactFollowUp::None => Ok(()),
            CompactFollowUp::ContextHandoff {
                target_selection,
                had_source,
            } => {
                self.complete_compact_handoff(succeeded, target_selection, had_source, agent)
                    .await
            }
        }
    }

    fn apply_compact_view_event(&mut self, event: super::ViewModelEvent) {
        if let Some(phase) = event.activity_phase() {
            self.turn.set_activity_phase(phase);
        }
        if let Some(entry) = self.record_agent_event(event) {
            self.insert_entry(&entry);
        }
    }

    fn finish_compact_ui(&mut self, outcome: CompactionUiOutcome) {
        let status = outcome.status_label();
        self.apply_compact_view_event(compact_finished_event(outcome));
        self.end_busy_ui();
        self.turn.stop_loading();
        self.set_status(status);
    }
}
