//! Main-loop compact job.
//!
//! `/compact` and pre-turn auto-compact start this job. In-run auto-compact
//! stays inside the SDK turn and applies the same card events from
//! `compaction_display` / `event_adapter`.

use ratatui::DefaultTerminal;

use crate::app::interactive_runtime::CompactTaskPoll;

use super::{
    compaction_display::CompactionUiOutcome,
    context_handoff::AfterHandoff,
    event_adapter::{compact_finished_event, compact_started_event},
    idle_input::HeldTurnWait,
    App, InteractiveModelSelection, InteractiveRuntime, ViewModelEvent,
};

/// Work to run after the current compact job settles.
pub(super) enum CompactFollowUp {
    None,
    ContextHandoff {
        target_selection: Option<InteractiveModelSelection>,
        had_source: bool,
        after: AfterHandoff,
    },
}

impl Default for CompactFollowUp {
    fn default() -> Self {
        Self::None
    }
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

    /// User cancel (`esc`). Parks compact-held turns for take-back and finishes
    /// a handoff follow-up as "not compacted".
    pub(super) async fn cancel_compact(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !agent.is_compacting() {
            return Ok(false);
        }
        agent.abort_compact_task().await;
        self.finish_compact_ui(CompactionUiOutcome::Cancelled);
        let follow_up = std::mem::take(&mut self.compact_follow_up);
        self.apply_compact_follow_up(follow_up, false, terminal, agent)
            .await?;
        Ok(true)
    }

    /// Drop the job and its follow-up. Used by `/new` and process exit.
    pub(super) async fn abort_compact(&mut self, agent: &mut InteractiveRuntime) -> bool {
        let started =
            agent.is_compacting() || !matches!(self.compact_follow_up, CompactFollowUp::None);
        agent.abort_compact_task().await;
        self.compact_follow_up = CompactFollowUp::None;
        if started {
            self.finish_compact_ui(CompactionUiOutcome::Cancelled);
        }
        started
    }

    pub(super) async fn poll_compact(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(poll) = agent.poll_compact_task().await else {
            return Ok(false);
        };
        let (outcome, succeeded) = match poll {
            CompactTaskPoll::Cancelled => (CompactionUiOutcome::Cancelled, false),
            CompactTaskPoll::Finished(Ok(Some(outcome))) => {
                (CompactionUiOutcome::from_sdk_outcome(&outcome), true)
            }
            CompactTaskPoll::Finished(Ok(None)) => (CompactionUiOutcome::unchanged(), false),
            CompactTaskPoll::Finished(Err(err)) => {
                (CompactionUiOutcome::failed(err.to_string()), false)
            }
        };
        let cancelled = matches!(outcome, CompactionUiOutcome::Cancelled);
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        self.finish_compact_ui(outcome);
        let follow_up = std::mem::take(&mut self.compact_follow_up);
        if !cancelled {
            self.promote_compact_holds();
        }
        self.apply_compact_follow_up(follow_up, succeeded, terminal, agent)
            .await?;
        Ok(true)
    }

    async fn apply_compact_follow_up(
        &mut self,
        follow_up: CompactFollowUp,
        succeeded: bool,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match follow_up {
            CompactFollowUp::None => Ok(()),
            CompactFollowUp::ContextHandoff {
                target_selection,
                had_source,
                after,
            } => {
                self.complete_compact_handoff(
                    succeeded,
                    target_selection,
                    had_source,
                    after,
                    terminal,
                    agent,
                )
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

    pub(super) fn promote_compact_holds(&mut self) {
        for held in &mut self.held_turns {
            if held.wait == HeldTurnWait::Compact {
                held.wait = HeldTurnWait::Ready;
            }
        }
    }
}
