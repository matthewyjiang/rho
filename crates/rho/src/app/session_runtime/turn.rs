//! One model turn: the prompt's run, saved as it goes, with what it reports
//! sent to the host as [`SessionEvent`]s.
//!
//! The turn owns everything that does not depend on the host: the step
//! checkpoints and the settled save, the session hooks, and the background
//! work (subagents, processes, workflows) that outlives prompts. The host
//! shows the events and answers the requests among them; see
//! [`super::events`].

use rho_sdk::{model::ToolCall, RunOutcome, UserInput};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::{
    delegation::{BackgroundSources, Delegation, OfferedDisplays},
    events::{SessionEvent, TurnControl},
    persist::TurnCheckpoint,
    prompt_gate::ActiveWork,
    pump::{pump, AppliedDisplays, ApprovalSource, EventSource, PumpSources},
    title::TitleState,
    PromptScope, QuestionSupport, SessionRuntime,
};
use crate::session::workspace_checkpoint::CheckpointOutcome;
use crate::{app::subagent_manager::SubagentManager, config::Config};

/// A model turn: `model` is what the run sends, `display` the prompt as the
/// user sent it, which the turn saves for display. `calls` run as one host
/// batch before the first model request.
pub(crate) struct Turn {
    pub(crate) display: UserInput,
    pub(crate) model: UserInput,
    pub(crate) calls: Vec<ToolCall>,
}

/// What a turn starts its run from: a prompt, or the history the session
/// was reopened with, as a workflow node continues its saved session after
/// an unclean exit.
pub(crate) enum TurnStart {
    Prompt(Turn),
    /// Continue the history the session was opened with; see
    /// [`rho_sdk::Session::continue_history`]. The turn inserts no user
    /// message and asks for no title.
    Continue,
}

impl From<Turn> for TurnStart {
    fn from(turn: Turn) -> Self {
        Self::Prompt(turn)
    }
}

/// What the run is started with, once the turn saved its prompt.
enum RunInput {
    Prompt {
        model: UserInput,
        calls: Vec<ToolCall>,
    },
    Continue,
}

/// How a turn talks to its host. Keep the control sender until the turn
/// returns: a turn whose control channel closes fails, since nothing can
/// answer its requests any more.
pub(crate) struct TurnIo {
    /// Where the turn reports what happens. Unbounded on purpose: the run
    /// never waits for a slow host to take an event, as the pump that
    /// forwarded events itself once did. A host that buffers per subscriber,
    /// such as the daemon's ring buffer, bounds its own backlog.
    pub(crate) events: UnboundedSender<SessionEvent>,
    pub(crate) control: UnboundedReceiver<TurnControl>,
}

impl TurnIo {
    /// Reports `event`. A host that stopped listening has failed the turn
    /// through [`TurnControl::HostFailed`] or is going away, so a closed
    /// channel is not an error here.
    pub(super) fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }
}

/// How a turn ended, when it did not fail.
#[derive(Debug)]
pub(crate) enum TurnOutcome {
    /// The run finished and the turn was saved; the outcome carries its
    /// stop reason and reply.
    Completed(RunOutcome),
    /// The run was cancelled; what it did so far was saved.
    Cancelled,
}

impl TurnOutcome {
    /// Whether the turn reached the model's final answer, including one the
    /// provider truncated or refused. Cancelled and step-limited turns did
    /// not.
    pub(super) fn answered(&self) -> bool {
        match self {
            Self::Completed(outcome) => match outcome.stop_reason() {
                rho_sdk::StopReason::EndTurn
                | rho_sdk::StopReason::MaxTokens
                | rho_sdk::StopReason::Refusal => true,
                rho_sdk::StopReason::MaxSteps => false,
                // `StopReason` is non-exhaustive; hosts report a reason a
                // newer SDK adds as an ended turn.
                _ => true,
            },
            Self::Cancelled => false,
        }
    }
}

impl SessionRuntime {
    /// Runs `turn` to its end within `prompt`, reporting through `io`.
    /// Failures are saved with what the turn showed and reported to the
    /// session's hooks before they return; one on the provider's
    /// credentials has the next prompt renew them.
    /// The first turn this runtime completes with the model's answer asks
    /// the title agent `process_config` configures to name the session, for
    /// the host to take with [`Self::take_title`]. Hosts run turns through
    /// [`Self::run_prompt_turn`], so none skips the plan handoff.
    pub(super) async fn run_turn(
        &mut self,
        prompt: &PromptScope,
        start: impl Into<TurnStart>,
        io: TurnIo,
        process_config: &Config,
    ) -> anyhow::Result<TurnOutcome> {
        let start = start.into();
        let title_input = match (&self.title, &start) {
            (TitleState::Unasked, TurnStart::Prompt(turn)) => Some(turn.display.clone()),
            (TitleState::Unasked, TurnStart::Continue)
            | (TitleState::Pending(_) | TitleState::Asked, _) => None,
        };
        let result = self.run_delegated(prompt, start, io).await;
        self.note_credential_failure(&result);
        if let Some(user) = title_input.filter(|_| result.as_ref().is_ok_and(TurnOutcome::answered))
        {
            self.request_title(process_config, &user);
        }
        result
    }

    /// Runs `turn` beside the session's background work.
    async fn run_delegated(
        &mut self,
        prompt: &PromptScope,
        start: TurnStart,
        mut io: TurnIo,
    ) -> anyhow::Result<TurnOutcome> {
        let stored = self.stored()?.cloned();
        let mut delegation = Delegation::attach(
            &self.built.session,
            BackgroundSources::of(&self.built.tools),
            self.background_wait(),
            stored.as_ref(),
            self.storage.persistence(),
            &mut self.held_notices,
        )?;
        let displays = delegation.offered_displays();
        // Children outlive a cancelled, failed, or step-limited prompt, as they
        // do across TUI turns: T3 Code cancels the open prompt to send each
        // mid-turn message. The next prompt waits for and delivers them and
        // their undelivered notices; only tearing the session down stops
        // them.
        let result = Delegation::drive(
            &mut delegation,
            self.drive_prompt(prompt, start, displays, &mut io),
        )
        .await;
        self.held_notices = delegation.hold_notices();
        result
    }

    async fn drive_prompt(
        &mut self,
        prompt: &PromptScope,
        start: TurnStart,
        displays: OfferedDisplays,
        io: &mut TurnIo,
    ) -> anyhow::Result<TurnOutcome> {
        let (checkpoint, input) = match start {
            TurnStart::Prompt(Turn {
                display,
                model,
                calls,
            }) => (
                self.install_turn_checkpoint(&display, &model),
                RunInput::Prompt { model, calls },
            ),
            TurnStart::Continue => (self.install_continuation_checkpoint(), RunInput::Continue),
        };
        let checkpoint = checkpoint?;
        let result = async {
            // The workspace checkpoint opens first, while the conversation's
            // leaf is still the one a rewind of this turn returns to.
            let paused = self.begin_workspace_checkpoint()?;
            // The prompt is durable before anything runs, so a host that
            // dies mid-turn still replays it.
            let inserted = self.insert_user_message(&checkpoint)?;
            io.emit(SessionEvent::MessageInserted(inserted));
            for text in self.reopen_notice.take().into_iter().chain(paused) {
                io.emit(SessionEvent::Notice { text });
            }
            self.drive_run(prompt, &checkpoint, input, displays, io)
                .await
        }
        .await;
        let ended = match &result {
            Ok(TurnOutcome::Completed(_)) => CheckpointOutcome::Completed,
            Ok(TurnOutcome::Cancelled) => CheckpointOutcome::Cancelled,
            Err(_) => CheckpointOutcome::Failed,
        };
        for text in self.finalize_workspace_checkpoint(ended) {
            io.emit(SessionEvent::Notice { text });
        }
        // The run is over, so the session is idle again.
        if let Err(error) = self.built.session.set_checkpoint_store(None) {
            tracing::warn!(%error, "could not remove turn checkpoint store");
        }
        result
    }

    async fn drive_run(
        &mut self,
        prompt: &PromptScope,
        checkpoint: &TurnCheckpoint,
        input: RunInput,
        displays: OfferedDisplays,
        io: &mut TurnIo,
    ) -> anyhow::Result<TurnOutcome> {
        let started = match input {
            RunInput::Prompt { model, calls } if calls.is_empty() => {
                self.built.session.start(model).await
            }
            RunInput::Prompt { model, calls } => {
                self.built.session.start_with_tool_calls(model, calls).await
            }
            RunInput::Continue => self.built.session.continue_history().await,
        };
        let mut run = match started {
            Ok(run) => run,
            Err(error) => {
                self.dispatch_failed(&error.to_string());
                return Err(error.into());
            }
        };
        prompt.activate(ActiveWork::model_run(&run));
        io.emit(SessionEvent::RunStarted);
        let cancel = run.cancellation_handle();
        let mut approval_receiver = self.built.approval_receiver.take();
        // Plan approvals belong to the tool set, which outlives this prompt
        // and is swapped by an agent rebind, so they are handed back after.
        let mut plan_approvals = self.built.tools.take_plan_permission_requests();
        // Bound for this run, and only when a person can be asked: a
        // subagent of an unbound parent gets no questionnaire tool. The run
        // stays open while its subagents work, or, where the host starts
        // turns of its own, while one of them has a question open.
        let mut child_questions = match self.build.host.questions {
            QuestionSupport::Available => self
                .built
                .tools
                .subagents()
                .map(SubagentManager::bind_host_input),
            QuestionSupport::Unavailable => None,
        };
        let pumped = pump(
            PumpSources {
                cancel,
                // A host that cannot ask a person gets no questions; the
                // turn declines them.
                questions: self.build.host.questions,
                events: EventSource::Run(&mut run),
                approvals: ApprovalSource::Receiver(approval_receiver.as_mut()),
                plan_approvals: plan_approvals.as_mut(),
                displays: Some(AppliedDisplays {
                    offered: displays,
                    checkpoint,
                }),
                hooks: self.built.runtime.hooks(),
                session: self.built.session.id(),
                child_questions: child_questions.as_mut(),
            },
            io,
        )
        .await;
        // Nobody listens between runs: a subagent still working asks
        // nothing rather than queueing a question for a later turn.
        if child_questions.take().is_some() {
            if let Some(manager) = self.built.tools.subagents() {
                manager.unbind_host_input();
            }
        }
        self.built
            .tools
            .restore_plan_permission_requests(plan_approvals);
        self.built.approval_receiver = approval_receiver;
        if let Err(error) = pumped {
            run.cancel();
            let _ = run.outcome().await;
            io.emit(SessionEvent::RunClosed);
            self.persist_failed_turn(checkpoint);
            self.dispatch_failed(&error.to_string());
            return Err(error);
        }
        let outcome = run.outcome().await;
        // The committed turn is the settled context; in-run updates were estimates.
        io.emit(SessionEvent::RunFinished {
            context: self.built.session.context_estimate(),
        });
        match outcome {
            Ok(outcome) => {
                if let Err(error) = self.settle_turn(checkpoint, &outcome) {
                    self.dispatch_failed(&error.to_string());
                    return Err(error);
                }
                self.completed_runs = self.completed_runs.saturating_add(1);
                self.built
                    .runtime
                    .hooks()
                    .session_completed(self.built.session.id(), self.completed_runs);
                Ok(TurnOutcome::Completed(outcome))
            }
            Err(rho_sdk::Error::Cancelled) => {
                if let Err(error) = self.persist_turn(checkpoint, /*assistant_text*/ None) {
                    self.dispatch_failed(&error.to_string());
                    return Err(error);
                }
                Ok(TurnOutcome::Cancelled)
            }
            Err(error) => {
                self.persist_failed_turn(checkpoint);
                self.dispatch_failed(&error.to_string());
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
