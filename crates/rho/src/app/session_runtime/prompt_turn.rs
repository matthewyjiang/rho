//! A prompt's model turns as one unit: the turn, the plan handoff it may
//! have asked for, and the implementation turn an approved plan starts.
//!
//! Every host runs a prompt's model turn through
//! [`SessionRuntime::run_prompt_turn`], so each gets the same sequence. The
//! runtime decides the handoff: once the turn that proposed a plan ends, an
//! approved plan switches the session's permission mode and the prompt goes
//! on with the implementation turn, as the TUI queues it right after the
//! proposal. The host only shows each turn through IO of its own and
//! reports the handoff, through its [`TurnView`].

use std::future::Future;

use rho_sdk::UserInput;

use super::{PromptScope, SessionRuntime, Turn, TurnIo, TurnOutcome, TurnStart};
use crate::{
    config::Config,
    tools::plan_exit::{PlanExitResolution, ProposalTurn},
};

const IMPLEMENT_PLAN: &str = "Implement the approved plan now. Follow the plan and verify the changes; the host has switched this session's permission mode.";

/// How a host shows a prompt's turns. It does not own the runtime:
/// [`SessionRuntime::run_prompt_turn`] lends it the runtime to read as each
/// turn opens and as the handoff is reported.
///
/// [`Self::open_turn`] is called once per turn. It returns fresh [`TurnIo`]
/// for the turn and the future that shows the turn through it: one that
/// takes the turn's events and answers its requests until the turn drops its
/// event sender, as it does when it returns, and keeps the control sender
/// until then. That future runs beside the turn, so it may borrow the view
/// but not the runtime; read what it needs of the runtime up front. What
/// showing the turn came to, such as an update the client missed, stays in
/// the view; it never changes how the turn ended, since the turn is saved
/// either way.
pub(crate) trait TurnView {
    /// The IO the next turn reports through, and the future that shows it.
    fn open_turn<'v>(
        &'v mut self,
        runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v, Self>);

    /// Shows how the handoff went, before the prompt goes on. `runtime` is
    /// in the mode the handoff left it.
    fn report_plan(
        &mut self,
        runtime: &SessionRuntime,
        process_config: &Config,
        report: PlanReport,
    ) -> impl Future<Output = ()> + Send;
}

/// What a host shows of a plan handoff. A turn that answered no plan has
/// none.
#[derive(Debug)]
pub(crate) enum PlanReport {
    /// The session kept its mode, for the reason `notice` tells the user.
    /// The prompt ends with the turn.
    Kept { notice: String },
    /// The session switched modes; the implementation turn runs next.
    Switched,
}

/// How a prompt goes on after the turn that may have answered
/// `exit_plan_mode`.
enum PlanHandoff {
    /// The turn answered no plan; the prompt ends as the turn did.
    Unanswered(anyhow::Result<TurnOutcome>),
    /// An answered plan did not switch the mode: it was not approved, the
    /// turn did not complete, or the switch failed, as `notice` says. The
    /// prompt ends as the proposing turn did, in the mode it had, except
    /// that a cancel which landed before the switch ends it
    /// `Ok(TurnOutcome::Cancelled)`; a failed turn stays failed.
    Kept {
        notice: String,
        ended: anyhow::Result<TurnOutcome>,
    },
    /// The session switched modes. This is the prompt's next turn, whose
    /// end is the prompt's.
    Switched(Turn),
}

impl SessionRuntime {
    /// Runs `turn` within `prompt`, then hands off the plan it may have
    /// answered: an approved plan on an answered turn switches the session's
    /// mode and runs the implementation turn in the same prompt. `view`
    /// shows both turns and reports the handoff between them. `turn` is a
    /// [`Turn`], or [`TurnStart::Continue`] to continue restored history.
    ///
    /// Returns the prompt's end: the implementation turn's after a switch,
    /// else the first turn's, which a cancel that landed before the switch
    /// turns into `Ok(TurnOutcome::Cancelled)`.
    pub(crate) async fn run_prompt_turn(
        &mut self,
        view: &mut impl TurnView,
        prompt: &PromptScope,
        turn: impl Into<TurnStart>,
        process_config: &Config,
    ) -> anyhow::Result<TurnOutcome> {
        let ended = self
            .show_turn(view, prompt, turn.into(), process_config)
            .await;
        match self.hand_off_plan(prompt, ended, process_config).await {
            PlanHandoff::Unanswered(ended) => ended,
            PlanHandoff::Kept { notice, ended } => {
                view.report_plan(self, process_config, PlanReport::Kept { notice })
                    .await;
                ended
            }
            PlanHandoff::Switched(implementation) => {
                view.report_plan(self, process_config, PlanReport::Switched)
                    .await;
                self.show_turn(view, prompt, implementation.into(), process_config)
                    .await
            }
        }
    }

    /// Runs `turn` beside `view` showing it.
    async fn show_turn(
        &mut self,
        view: &mut impl TurnView,
        prompt: &PromptScope,
        turn: TurnStart,
        process_config: &Config,
    ) -> anyhow::Result<TurnOutcome> {
        let (io, showing) = view.open_turn(self);
        let (ended, ()) = tokio::join!(self.run_turn(prompt, turn, io, process_config), showing);
        ended
    }

    /// Applies the answer to `exit_plan_mode` that `prompt`'s turn, which
    /// ended as `proposal`, recorded. Called once the host has shown that
    /// turn. `proposal` is how the turn itself ended: an update the host
    /// failed to show does not change it, since the turn was saved either
    /// way. A cancel that lands before the switch commits, such as during
    /// the host's last update of the proposal, still cancels the handoff; a
    /// later one stops only the implementation turn.
    async fn hand_off_plan(
        &mut self,
        prompt: &PromptScope,
        proposal: anyhow::Result<TurnOutcome>,
        process_config: &Config,
    ) -> PlanHandoff {
        let Some(decision) = self.built.tools.take_plan_exit_decision() else {
            return PlanHandoff::Unanswered(proposal);
        };
        let cancelled = prompt.is_cancelled();
        let turn = match &proposal {
            Ok(outcome) if outcome.answered() && !cancelled => ProposalTurn::Completed,
            Ok(_) | Err(_) => ProposalTurn::Unfinished,
        };
        let target = match decision.resolve(turn, self.permission.mode) {
            PlanExitResolution::Apply(target) => target,
            PlanExitResolution::Keep(reason) => {
                let ended = if cancelled {
                    proposal.map(|_| TurnOutcome::Cancelled)
                } else {
                    proposal
                };
                return PlanHandoff::Kept {
                    notice: reason.to_owned(),
                    ended,
                };
            }
        };
        if let Err(error) = self.set_permission_mode(target, process_config).await {
            return PlanHandoff::Kept {
                notice: format!("could not execute approved plan: {error:#}"),
                ended: proposal,
            };
        }
        // A permission prompt carries no feedback, so the decision has none.
        let input = UserInput::text(IMPLEMENT_PLAN);
        PlanHandoff::Switched(Turn {
            display: input.clone(),
            model: input,
            calls: Vec::new(),
        })
    }
}

#[cfg(test)]
#[path = "prompt_turn_tests.rs"]
mod tests;
