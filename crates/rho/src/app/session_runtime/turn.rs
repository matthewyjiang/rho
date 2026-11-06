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
    events::{RunEnd, SessionEvent, TurnControl},
    persist::TurnCheckpoint,
    pump::{pump, AppliedDisplays, ApprovalSource, EventSource, PumpSources},
    title::TitleState,
    PromptScope, SessionRuntime,
};
use crate::{
    app::headless_delegation::{BackgroundSources, HeadlessDelegation, OfferedDisplays},
    config::Config,
};

/// A model turn: `model` is what the run sends, `display` the prompt as the
/// user sent it, which the turn saves for display. `calls` run as one host
/// batch before the first model request.
pub(crate) struct Turn {
    pub(crate) display: UserInput,
    pub(crate) model: UserInput,
    pub(crate) calls: Vec<ToolCall>,
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
    /// the host to take with [`Self::take_title`].
    pub(crate) async fn run_turn(
        &mut self,
        prompt: &PromptScope,
        turn: Turn,
        io: TurnIo,
        process_config: &Config,
    ) -> anyhow::Result<TurnOutcome> {
        let title_input = match self.title {
            TitleState::Unasked => Some(turn.display.clone()),
            TitleState::Pending(_) | TitleState::Asked => None,
        };
        let result = self.run_delegated(prompt, turn, io).await;
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
        turn: Turn,
        mut io: TurnIo,
    ) -> anyhow::Result<TurnOutcome> {
        let stored = self.stored()?.clone();
        let mut delegation = HeadlessDelegation::attach(
            &self.built.session,
            BackgroundSources::of(&self.built.tools),
            Some(&stored),
            &mut self.held_notices,
        )?;
        delegation.set_background_wait(self.build.background_wait);
        let displays = delegation.offered_displays();
        // Children outlive a cancelled, failed, or step-limited prompt, as they
        // do across TUI turns: T3 Code cancels the open prompt to send each
        // mid-turn message. The next prompt waits for and delivers them and
        // their undelivered notices; only closing or replacing the session
        // stops them.
        let result = HeadlessDelegation::drive(
            &mut delegation,
            self.drive_prompt(prompt, turn, displays, &mut io),
        )
        .await;
        self.held_notices = delegation.hold_notices();
        result
    }

    async fn drive_prompt(
        &mut self,
        prompt: &PromptScope,
        turn: Turn,
        displays: OfferedDisplays,
        io: &mut TurnIo,
    ) -> anyhow::Result<TurnOutcome> {
        let Turn {
            display,
            model,
            calls,
        } = turn;
        let checkpoint = self.install_turn_checkpoint(&display, &model)?;
        // The prompt is durable before anything runs, so a host that dies
        // mid-turn still replays it.
        let result = match self.insert_user_message(&checkpoint) {
            Ok(inserted) => {
                io.emit(SessionEvent::MessageInserted(inserted));
                if let Some(text) = self.reopen_notice.take() {
                    io.emit(SessionEvent::Notice { text });
                }
                self.drive_run(prompt, &checkpoint, model, calls, displays, io)
                    .await
            }
            Err(error) => Err(error),
        };
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
        model: UserInput,
        calls: Vec<ToolCall>,
        displays: OfferedDisplays,
        io: &mut TurnIo,
    ) -> anyhow::Result<TurnOutcome> {
        let started = if calls.is_empty() {
            self.built.session.start(model).await
        } else {
            self.built.session.start_with_tool_calls(model, calls).await
        };
        let mut run = match started {
            Ok(run) => run,
            Err(error) => {
                self.dispatch_failed(&error.to_string());
                return Err(error.into());
            }
        };
        prompt.activate(run.cancellation_handle());
        io.emit(SessionEvent::RunStarted);
        let cancel = run.cancellation_handle();
        let mut approval_receiver = self.built.approval_receiver.take();
        // Plan approvals belong to the tool set, which outlives this prompt
        // and is swapped by an agent rebind, so they are handed back after.
        let mut plan_approvals = self.built.tools.take_plan_permission_requests();
        let pumped = pump(
            PumpSources {
                cancel,
                // A host that cannot ask a person gets no questions; the
                // turn declines them.
                questions: self.build.questions,
                events: EventSource::Run(&mut run),
                approvals: ApprovalSource::Receiver(approval_receiver.as_mut()),
                plan_approvals: plan_approvals.as_mut(),
                displays: Some(AppliedDisplays {
                    offered: displays,
                    checkpoint,
                }),
            },
            io,
        )
        .await;
        self.built
            .tools
            .restore_plan_permission_requests(plan_approvals);
        self.built.approval_receiver = approval_receiver;
        if let Err(error) = pumped {
            run.cancel();
            let _ = run.outcome().await;
            io.emit(SessionEvent::RunClosed {
                ending: RunEnd::Failed(error.to_string()),
            });
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
