use std::{future::Future, pin::Pin};

use rho_sdk::{
    model::{ContextUsage, Message},
    Error, HostInputId, HostInputResponse, Run, RunEvent, RunOutcome, UserInput,
};

use super::interactive_state::{state_after_event, InteractiveState, RunPhase, RunState};

pub(crate) type SteeringAcceptanceFuture =
    Pin<Box<dyn Future<Output = Result<rho_sdk::SteeringId, Error>> + Send>>;
pub(crate) type SteeringRetractionFuture =
    Pin<Box<dyn Future<Output = Result<rho_sdk::SteeringRetraction, Error>> + Send>>;

pub(crate) struct PendingTurn {
    model_user: Message,
    display_user: Option<Vec<Message>>,
    history_start: usize,
    checkpoints: Vec<DisplayCheckpoint>,
}

/// Boundary checkpoints own the full accepted prefix, not positions in mutable
/// SDK history. Compaction checkpoints only establish the next history baseline.
/// The SDK does not expose discarded precompaction history, so only prefixes
/// captured before compaction can survive it; later history is appended verbatim.
struct DisplayCheckpoint {
    revision: rho_sdk::Revision,
    history: Vec<Message>,
    replacement: Option<(usize, Message)>,
}

impl PendingTurn {
    pub(crate) fn new(
        model_user: Message,
        display_user: Option<Vec<Message>>,
        history_start: usize,
    ) -> Self {
        Self {
            model_user,
            display_user,
            history_start,
            checkpoints: Vec::new(),
        }
    }

    /// Acceptance checkpoints the input before the host records its display.
    /// Bind to that occurrence, never to a later text-equivalent user message.
    pub(crate) fn record_boundary_display(
        &mut self,
        model: Message,
        display: Message,
        history: &[Message],
        revision: rho_sdk::Revision,
    ) {
        let Some(index) = history.iter().rposition(|message| message == &model) else {
            return;
        };
        let history = &history[..=index];
        if self.checkpoints.iter().any(|checkpoint| {
            checkpoint.revision == revision
                && checkpoint.history == history
                && checkpoint
                    .replacement
                    .as_ref()
                    .is_some_and(|(at, _)| *at == index)
        }) {
            return;
        }
        self.checkpoints.push(DisplayCheckpoint {
            revision,
            history: history.to_vec(),
            replacement: Some((index, display)),
        });
    }

    /// A queued compaction event may precede an already acknowledged input.
    /// Insert its baseline before that input rather than discarding its capture.
    pub(crate) fn checkpoint_compaction(
        &mut self,
        history: &[Message],
        revision: rho_sdk::Revision,
    ) -> Vec<Message> {
        let index = self
            .checkpoints
            .iter()
            .position(|checkpoint| checkpoint.revision > revision)
            .unwrap_or(self.checkpoints.len());
        self.checkpoints.insert(
            index,
            DisplayCheckpoint {
                revision,
                history: history.to_vec(),
                replacement: None,
            },
        );
        self.accumulate(&self.checkpoints[..=index], None)
    }

    pub(crate) fn display_tail(
        &self,
        history: &[Message],
        outcome: Option<&RunOutcome>,
    ) -> Vec<Message> {
        let mut display = self.accumulate(&self.checkpoints, Some(history));
        if self.checkpoints.is_empty() && history.get(self.history_start) != Some(&self.model_user)
        {
            display.extend(
                outcome
                    .filter(|outcome| !outcome.text().is_empty())
                    .map(|outcome| Message::assistant_text(outcome.text().to_string())),
            );
        }
        display
    }

    fn accumulate(
        &self,
        checkpoints: &[DisplayCheckpoint],
        final_history: Option<&[Message]>,
    ) -> Vec<Message> {
        let mut display = self
            .display_user
            .clone()
            .unwrap_or_else(|| vec![self.model_user.clone()]);
        let mut previous: Option<&[Message]> = None;
        for checkpoint in checkpoints {
            if let Some((index, replacement)) = &checkpoint.replacement {
                if let Some(tail) = self.appended_history(previous, &checkpoint.history) {
                    let start = checkpoint.history.len() - tail.len();
                    for (offset, message) in tail.iter().enumerate() {
                        display.push(
                            if start + offset == *index {
                                replacement
                            } else {
                                message
                            }
                            .clone(),
                        );
                    }
                } else {
                    // Cancellation can prevent consumption of an older compaction
                    // event. Its missing baseline must not erase accepted input.
                    display.push(replacement.clone());
                }
            }
            previous = Some(&checkpoint.history);
        }
        if let Some(tail) =
            final_history.and_then(|history| self.appended_history(previous, history))
        {
            display.extend_from_slice(tail);
        }
        display
    }

    /// Append only a verified extension of the last checkpoint or initial input.
    fn appended_history<'a>(
        &self,
        previous: Option<&[Message]>,
        history: &'a [Message],
    ) -> Option<&'a [Message]> {
        match previous {
            Some(previous) => history.strip_prefix(previous),
            None => (history.get(self.history_start) == Some(&self.model_user))
                .then(|| &history[self.history_start + 1..]),
        }
    }
}

#[cfg(test)]
#[path = "interactive_run_controller_tests.rs"]
mod tests;

pub(crate) struct FinishedRun {
    pub(crate) outcome: Result<RunOutcome, Error>,
    pub(crate) pending_turn: Option<PendingTurn>,
}

/// Exact durability reached before a turn succeeds, fails, or rolls back.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum DisplayCommit {
    #[default]
    Unsaved,
    Checkpoint(Vec<Message>),
    Complete,
}

#[derive(Default)]
pub(crate) struct InteractiveRunController {
    active: Option<Run>,
    state: InteractiveState,
    pending_turn: Option<PendingTurn>,
    pending_context_usage: Option<ContextUsage>,
    cumulative_input_tokens: u64,
    step_input_token_baseline: u64,
    last_turn_display_commit: DisplayCommit,
}

impl InteractiveRunController {
    pub(crate) fn reset_display_committed(&mut self) {
        self.last_turn_display_commit = DisplayCommit::Unsaved;
    }

    pub(crate) fn mark_display_committed(&mut self) {
        self.last_turn_display_commit = DisplayCommit::Complete;
    }

    pub(crate) fn mark_display_checkpoint(&mut self, display: Vec<Message>) {
        self.last_turn_display_commit = DisplayCommit::Checkpoint(display);
    }

    pub(crate) fn take_last_turn_display_commit(&mut self) -> DisplayCommit {
        std::mem::take(&mut self.last_turn_display_commit)
    }

    pub(crate) fn checkpoint_compaction(
        &mut self,
        snapshot: &rho_sdk::SessionSnapshot,
    ) -> Vec<Message> {
        self.pending_turn.as_mut().map_or_else(Vec::new, |turn| {
            turn.checkpoint_compaction(snapshot.history(), snapshot.revision())
        })
    }

    pub(crate) fn record_boundary_display(
        &mut self,
        model: Message,
        display: Message,
        snapshot: &rho_sdk::SessionSnapshot,
    ) {
        if let Some(turn) = &mut self.pending_turn {
            turn.record_boundary_display(model, display, snapshot.history(), snapshot.revision());
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn state(&self) -> InteractiveState {
        self.state
    }

    pub(crate) fn begin_provider_switch(&mut self) -> Result<(), Error> {
        self.state = super::interactive_state::begin_provider_switch(self.state)?;
        Ok(())
    }

    pub(crate) fn finish_transition(&mut self) {
        debug_assert!(matches!(self.state, InteractiveState::Transition(_)));
        self.state = InteractiveState::Idle;
    }

    pub(crate) fn begin(
        &mut self,
        run: Run,
        pending_turn: PendingTurn,
        context_usage: ContextUsage,
    ) -> Result<(), Error> {
        if self.state != InteractiveState::Idle || self.active.is_some() {
            return Err(Error::SessionBusy);
        }
        self.active = Some(run);
        self.pending_turn = Some(pending_turn);
        self.pending_context_usage = Some(context_usage);
        self.cumulative_input_tokens = 0;
        self.step_input_token_baseline = 0;
        self.state = InteractiveState::Run(RunState::Running(RunPhase::Model));
        Ok(())
    }

    pub(crate) async fn next_event(&mut self, context_window: Option<u64>) -> Option<RunEvent> {
        let event = self.active.as_mut()?.next_event().await;
        if let Some(event) = &event {
            self.observe_event(event, context_window);
        }
        event
    }

    pub(crate) fn cancel(&mut self) {
        let Some(run) = &self.active else {
            return;
        };
        let phase = match self.state {
            InteractiveState::Run(RunState::Running(phase) | RunState::Cancelling(phase)) => phase,
            InteractiveState::Run(RunState::WaitingForHostInput) => RunPhase::Tool,
            _ => RunPhase::Model,
        };
        run.cancel();
        self.state = InteractiveState::Run(RunState::Cancelling(phase));
    }

    pub(crate) fn request_steer(
        &mut self,
        input: UserInput,
    ) -> Result<SteeringAcceptanceFuture, Error> {
        let receipt = self
            .active
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts steering input".into(),
            })?
            .request_steer_retractable(input)?;
        self.state = InteractiveState::Run(RunState::Running(RunPhase::Steering));
        Ok(Box::pin(receipt))
    }

    pub(crate) fn request_steering_retraction(
        &self,
        id: rho_sdk::SteeringId,
    ) -> Result<SteeringRetractionFuture, Error> {
        let receipt = self
            .active
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts steering retractions".into(),
            })?
            .request_steering_retraction(id)?;
        Ok(Box::pin(receipt))
    }

    pub(crate) async fn respond(
        &mut self,
        request_id: HostInputId,
        response: HostInputResponse,
    ) -> Result<(), Error> {
        self.active
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts host input".into(),
            })?
            .respond(request_id, response)
            .await?;
        self.state = InteractiveState::Run(RunState::Running(RunPhase::Tool));
        Ok(())
    }

    pub(crate) async fn finish(&mut self) -> anyhow::Result<FinishedRun> {
        let mut run = self
            .active
            .take()
            .ok_or_else(|| anyhow::anyhow!("no active run"))?;
        let outcome = run.outcome().await;
        self.state = InteractiveState::Idle;
        Ok(FinishedRun {
            outcome,
            pending_turn: self.pending_turn.take(),
        })
    }

    pub(crate) fn take_context_usage(&mut self) -> Option<ContextUsage> {
        self.pending_context_usage.take()
    }

    pub(crate) fn note_context_usage(&mut self, usage: ContextUsage) {
        self.pending_context_usage = Some(usage);
    }

    pub(crate) fn note_manual_compaction(&mut self, context_window: Option<u64>) {
        self.note_context_usage(ContextUsage::unknown_after_compaction(context_window));
    }

    pub(crate) fn observe_event(&mut self, event: &RunEvent, context_window: Option<u64>) {
        self.state = state_after_event(self.state, event);
        match event {
            RunEvent::Started { .. } => {
                self.cumulative_input_tokens = 0;
                self.step_input_token_baseline = 0;
            }
            RunEvent::StepStarted {
                estimated_context_tokens,
                ..
            } => {
                self.step_input_token_baseline = self.cumulative_input_tokens;
                self.note_context_usage(ContextUsage::estimated(
                    *estimated_context_tokens,
                    context_window,
                ));
            }
            RunEvent::UsageUpdated { usage } => {
                if let Some(cumulative_tokens) = usage.inclusive_prompt_tokens() {
                    self.cumulative_input_tokens = cumulative_tokens;
                    let tokens = cumulative_tokens.saturating_sub(self.step_input_token_baseline);
                    let context_window = match (usage.context_window, context_window) {
                        (Some(reported), Some(configured)) => Some(reported.min(configured)),
                        (reported, configured) => reported.or(configured),
                    };
                    self.note_context_usage(ContextUsage::provider_reported(
                        tokens,
                        context_window,
                    ));
                }
            }
            RunEvent::CompactionCompleted { .. } => {
                self.note_context_usage(ContextUsage::unknown_after_compaction(context_window));
            }
            _ => {}
        }
    }
}
