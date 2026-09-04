//! Main-loop compact job.
//!
//! `/compact` and pre-turn auto-compact start this job. In-run auto-compact
//! stays inside the SDK turn and applies the same card events from
//! `compaction_display` / `event_adapter`.

use crate::app::interactive_runtime::CompactTaskPoll;

use super::{
    compaction_display::CompactionUiOutcome,
    event_adapter::{compact_finished_event, compact_started_event},
    send_confirm::SendSubmission,
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
    /// The compact job exclusively owns this approved submission until it
    /// settles; it is never also inserted into the editable prompt queue.
    Send(Box<SendSubmission>),
}

pub(super) enum ReadyFollowUp {
    Queued { allow_auto_compact: bool },
    Send(Box<SendSubmission>),
}

enum SettledSend {
    Ready(ReadyFollowUp),
    Cancelled(Box<SendSubmission>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompactSettlementIntent {
    Poll,
    UserCancelled,
}

fn should_start_compact_follow_ups(
    intent: CompactSettlementIntent,
    outcome_starts_follow_ups: bool,
) -> bool {
    matches!(intent, CompactSettlementIntent::Poll) && outcome_starts_follow_ups
}

fn settle_compact_send(submission: Box<SendSubmission>, starts_follow_ups: bool) -> SettledSend {
    if starts_follow_ups {
        SettledSend::Ready(ReadyFollowUp::Send(submission))
    } else {
        SettledSend::Cancelled(submission)
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
        self.begin_started_compact(follow_up);
        Ok(())
    }

    /// Starts compact while preserving the exact approved submission if task
    /// startup fails before the compact job can take ownership.
    pub(super) fn start_compact_send(
        &mut self,
        agent: &mut InteractiveRuntime,
        submission: Box<SendSubmission>,
    ) -> Result<(), (anyhow::Error, Box<SendSubmission>)> {
        if agent.is_compacting() {
            return Err((anyhow::anyhow!("already compacting context"), submission));
        }
        if let Err(error) = agent.begin_compact_task() {
            return Err((error, submission));
        }
        self.begin_started_compact(CompactFollowUp::Send(submission));
        Ok(())
    }

    fn begin_started_compact(&mut self, follow_up: CompactFollowUp) {
        self.pending.steering_prompts_mut().clear();
        self.pending_input_changed();
        self.set_status("compacting context");
        self.begin_compact_ui();
        self.turn.start_loading();
        self.apply_compact_view_event(compact_started_event());
        self.compact_follow_up = follow_up;
    }

    /// User cancel (`esc`). A compact that already finished still applies, but
    /// cancellation suppresses queued and send follow-ups.
    pub(super) async fn cancel_compact(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(poll) = agent.abort_compact_task().await else {
            return Ok(false);
        };
        self.settle_compact_poll(poll, CompactSettlementIntent::UserCancelled, agent)
            .await?;
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
        self.settle_compact_poll(poll, CompactSettlementIntent::Poll, agent)
            .await?;
        Ok(true)
    }

    async fn settle_compact_poll(
        &mut self,
        poll: CompactTaskPoll,
        intent: CompactSettlementIntent,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let outcome = match poll {
            CompactTaskPoll::Cancelled => CompactionUiOutcome::Cancelled,
            CompactTaskPoll::Finished(result) => CompactionUiOutcome::from_task_result(result),
        };
        let succeeded = matches!(outcome, CompactionUiOutcome::Completed(_));
        let starts_follow_ups =
            should_start_compact_follow_ups(intent, outcome.starts_follow_ups());
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        self.finish_compact_ui(outcome);
        let follow_up = std::mem::take(&mut self.compact_follow_up);
        self.apply_compact_follow_up(follow_up, succeeded, starts_follow_ups, agent)
            .await
    }

    async fn apply_compact_follow_up(
        &mut self,
        follow_up: CompactFollowUp,
        succeeded: bool,
        starts_follow_ups: bool,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match follow_up {
            CompactFollowUp::None => {
                if starts_follow_ups {
                    self.start_follow_ups = Some(ReadyFollowUp::Queued {
                        allow_auto_compact: false,
                    });
                }
                Ok(())
            }
            CompactFollowUp::ContextHandoff {
                target_selection,
                had_source,
            } => {
                if starts_follow_ups {
                    self.start_follow_ups = Some(ReadyFollowUp::Queued {
                        allow_auto_compact: false,
                    });
                }
                // A compact result may have committed just before Esc won the
                // cancellation race. Keep that result, but do not apply the
                // model-switch continuation the user just cancelled.
                self.complete_compact_handoff(
                    succeeded && starts_follow_ups,
                    target_selection,
                    had_source,
                    agent,
                )
                .await
            }
            CompactFollowUp::Send(submission) => {
                match settle_compact_send(submission, starts_follow_ups) {
                    SettledSend::Ready(ready) => self.start_follow_ups = Some(ready),
                    SettledSend::Cancelled(submission) => {
                        self.cancel_compact_send_submission(*submission, agent);
                    }
                }
                Ok(())
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

#[cfg(test)]
#[path = "compact_work_tests.rs"]
mod tests;
