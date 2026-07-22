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
    display_user: Option<Message>,
    history_start: usize,
}

impl PendingTurn {
    pub(crate) fn new(
        model_user: Message,
        display_user: Option<Message>,
        history_start: usize,
    ) -> Self {
        Self {
            model_user,
            display_user,
            history_start,
        }
    }

    pub(crate) fn model_user(&self) -> &Message {
        &self.model_user
    }

    pub(crate) fn display_user(&self) -> Option<&Message> {
        self.display_user.as_ref()
    }

    pub(crate) fn history_start(&self) -> usize {
        self.history_start
    }
}

pub(crate) struct FinishedRun {
    pub(crate) outcome: Result<RunOutcome, Error>,
    pub(crate) pending_turn: Option<PendingTurn>,
}

#[derive(Default)]
pub(crate) struct InteractiveRunController {
    active: Option<Run>,
    state: InteractiveState,
    pending_turn: Option<PendingTurn>,
    pending_context_usage: Option<ContextUsage>,
    cumulative_input_tokens: u64,
    step_input_token_baseline: u64,
}

impl InteractiveRunController {
    pub(crate) fn pending_turn(&self) -> Option<&PendingTurn> {
        self.pending_turn.as_ref()
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
            RunEvent::StepStarted { .. } => {
                self.step_input_token_baseline = self.cumulative_input_tokens;
            }
            RunEvent::UsageUpdated { usage } => {
                if let Some(cumulative_tokens) = usage.total_input_tokens() {
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
