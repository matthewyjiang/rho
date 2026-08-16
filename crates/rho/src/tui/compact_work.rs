//! Main-loop compaction: `/compact` and auto-compact share one job.
//!
//! Compact is not a model tool, but it uses the same card. The work runs off
//! the input loop so the composer stays idle input.

use std::time::Duration;

use ratatui::DefaultTerminal;

use crate::app::interactive_runtime::CompactTaskResult;

use super::{
    activity::ActivityPhase,
    compaction_display::{
        compaction_call_id, running_card, CompactionDisplayFacts, CompactionUiOutcome,
    },
    App, ChatMedia, Entry, InteractiveRuntime, ToolEntry, TurnPrompt, ViewModelEvent,
};

pub(super) struct PendingCompact {
    handle: tokio::task::JoinHandle<CompactTaskResult>,
}

pub(super) struct PendingCompactSubmission {
    pub(super) turn: TurnPrompt,
    pub(super) media: Vec<ChatMedia>,
}

impl App {
    pub(super) fn start_compact(&mut self, agent: &mut InteractiveRuntime) -> anyhow::Result<()> {
        if self.pending_compact.is_some() || agent.is_compacting() {
            self.notify_status("already compacting context");
            return Ok(());
        }
        self.last_compact_ok = false;
        let task = agent.begin_compact_task()?;
        self.pending.steering_prompts_mut().clear();
        self.pending_input_changed();
        self.set_status("compacting context");
        self.begin_compact_ui();
        self.turn.set_activity_phase(ActivityPhase::Compacting);
        self.turn.start_loading();
        self.turn.tool_started(compaction_call_id(), running_card());
        self.pending_compact = Some(PendingCompact {
            handle: tokio::spawn(task.run()),
        });
        Ok(())
    }

    pub(super) async fn cancel_compact(&mut self, agent: &mut InteractiveRuntime) -> bool {
        let Some(pending) = self.pending_compact.take() else {
            return false;
        };
        pending.handle.abort();
        let _ = pending.handle.await;
        agent.abort_compact_task();
        self.finish_compact_ui(
            CompactionUiOutcome::Cancelled,
            "context compaction cancelled",
        );
        true
    }

    pub(super) async fn poll_compact(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(pending) = self.pending_compact.as_mut() else {
            return Ok(false);
        };
        if !pending.handle.is_finished() {
            return Ok(false);
        }
        let Some(pending) = self.pending_compact.take() else {
            return Ok(false);
        };
        match pending.handle.await {
            Ok(result) => self.complete_compact(agent, result).await?,
            Err(error) if error.is_cancelled() => {
                agent.abort_compact_task();
                self.finish_compact_ui(
                    CompactionUiOutcome::Cancelled,
                    "context compaction cancelled",
                );
            }
            Err(error) => {
                agent.abort_compact_task();
                self.finish_compact_ui(
                    CompactionUiOutcome::Failed {
                        detail: error.to_string(),
                    },
                    "context compaction failed",
                );
            }
        }
        Ok(true)
    }

    /// Drive compact to completion with idle input. Used when a caller must
    /// finish compact before the next step, such as model handoff.
    pub(super) async fn drive_compact(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        terminal.draw(|frame| self.draw(frame))?;
        while self.pending_compact.is_some() {
            let timeout = if self.animation_active(std::time::Instant::now()) {
                Duration::from_millis(80)
            } else {
                Duration::from_secs(3600)
            };
            tokio::select! {
                event = self.terminal_session.as_mut().expect("terminal session initialized").next_event() => {
                    self.handle_terminal_event(event?, terminal, agent).await?;
                    self.flush_due_paste_burst();
                }
                _ = tokio::time::sleep(timeout) => {
                    self.flush_due_paste_burst();
                }
            }
            self.poll_compact(agent).await?;
            if self.should_quit {
                self.cancel_compact(agent).await;
                break;
            }
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(self.last_compact_succeeded())
    }

    pub(super) fn hold_turn_for_compact(&mut self, turn: TurnPrompt, media: Vec<ChatMedia>) {
        self.pending_compact_submissions
            .push_back(PendingCompactSubmission { turn, media });
    }

    pub(super) async fn release_pending_compact_submission(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if self.pending_compact.is_some() || self.input_ui.composer().blocks_held_turn_start() {
            return Ok(false);
        }
        let Some(PendingCompactSubmission { mut turn, media }) =
            self.pending_compact_submissions.pop_front()
        else {
            return Ok(false);
        };
        turn.skip_auto_compact = true;
        self.run_turn_sequence(turn, media, terminal, agent).await?;
        Ok(true)
    }

    pub(super) fn take_back_compact_held_turn(&mut self) -> bool {
        if !self.input_ui.text().is_empty() || !self.input_ui.attachments().is_empty() {
            return false;
        }
        let Some(PendingCompactSubmission { turn, media, .. }) =
            self.pending_compact_submissions.pop_back()
        else {
            return false;
        };
        self.restore_pending_prompt(super::QueuedPrompt {
            prompt: turn.model,
            display_prompt: turn.display,
            paste_segments: Vec::new(),
        });
        if media.is_empty() {
            self.set_status_quiet("");
        } else {
            self.notify_status("prompt returned to the composer; attach the files again");
        }
        true
    }

    async fn complete_compact(
        &mut self,
        agent: &mut InteractiveRuntime,
        result: CompactTaskResult,
    ) -> anyhow::Result<()> {
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        let (outcome, status) = match agent.complete_compact_task(result).await {
            Ok(Some(outcome)) => (
                CompactionUiOutcome::Completed(CompactionDisplayFacts::from_outcome(&outcome)),
                "context compacted",
            ),
            Ok(None) => (
                CompactionUiOutcome::Unchanged {
                    detail: "not enough conversation history to compact, or the model context window is unknown".into(),
                },
                "context not compacted",
            ),
            Err(err) => (
                CompactionUiOutcome::Failed {
                    detail: err.to_string(),
                },
                "context compaction failed",
            ),
        };
        self.finish_compact_ui(outcome, status);
        Ok(())
    }

    fn finish_compact_ui(&mut self, outcome: CompactionUiOutcome, status: &'static str) {
        self.last_compact_ok = matches!(outcome, CompactionUiOutcome::Completed(_));
        self.end_busy_ui();
        self.turn.stop_loading();
        let expanded = self.turn.tool_finished(&compaction_call_id());
        self.insert_entry(&Entry::Tool(ToolEntry {
            card: outcome.card(),
            expanded,
            image: None,
            started_at: None,
        }));
        self.set_status(status);
    }

    fn last_compact_succeeded(&self) -> bool {
        self.last_compact_ok
    }
}
