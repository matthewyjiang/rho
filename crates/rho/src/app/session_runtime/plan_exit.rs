//! Plan approval's handoff: once the turn that proposed a plan ends, an
//! approved plan switches the session's permission mode and the prompt goes
//! on with the implementation turn, as the TUI queues it right after the
//! proposal.

use rho_sdk::UserInput;

use super::{PromptScope, SessionRuntime, Turn, TurnOutcome};
use crate::{
    config::Config,
    permission::PermissionMode,
    tools::plan_exit::{PlanExitResolution, ProposalTurn},
};

const IMPLEMENT_PLAN: &str = "Implement the approved plan now. Follow the plan and verify the changes; the host has switched this session's permission mode.";

/// How a prompt goes on after the turn that may have answered
/// `exit_plan_mode`.
pub(crate) enum PlanHandoff {
    /// The prompt ends as `ended` says, in the mode it had. `ended` is the
    /// proposing turn's own end, except that a cancel which landed before
    /// the switch ends it `Ok(TurnOutcome::Cancelled)`; a failed turn stays
    /// failed. `notice`, for the host to show the user, says why an answered
    /// plan did not switch: it was not approved, the turn did not complete,
    /// or the switch failed. It is `None` when the turn answered no plan.
    Ended {
        notice: Option<String>,
        ended: anyhow::Result<TurnOutcome>,
    },
    /// The session is now in `mode`. Show `notice`, then run
    /// `implementation` as the prompt's next turn; its end is the prompt's.
    Switched {
        mode: PermissionMode,
        notice: String,
        implementation: Turn,
    },
}

impl SessionRuntime {
    /// Applies the answer to `exit_plan_mode` that `prompt`'s turn, which
    /// ended as `proposal`, recorded. Call once the host has shown that
    /// turn. `proposal` is how the turn itself ended: an update the host
    /// failed to show does not change it, since the turn was saved either
    /// way. A cancel that lands before the switch commits, such as during
    /// the host's last update of the proposal, still cancels the handoff; a
    /// later one stops only the implementation turn.
    pub(crate) async fn hand_off_plan(
        &mut self,
        prompt: &PromptScope,
        proposal: anyhow::Result<TurnOutcome>,
        process_config: &Config,
    ) -> PlanHandoff {
        let Some(decision) = self.built.tools.take_plan_exit_decision() else {
            return PlanHandoff::Ended {
                notice: None,
                ended: proposal,
            };
        };
        let cancelled = prompt.is_cancelled();
        let turn = match &proposal {
            Ok(outcome) if outcome.answered() && !cancelled => ProposalTurn::Completed,
            Ok(_) | Err(_) => ProposalTurn::Unfinished,
        };
        let target = match decision.resolve(turn, self.permission.mode) {
            PlanExitResolution::Apply(target) => target,
            PlanExitResolution::Keep(notice) => {
                let ended = if cancelled {
                    proposal.map(|_| TurnOutcome::Cancelled)
                } else {
                    proposal
                };
                return PlanHandoff::Ended {
                    notice: Some(notice.to_owned()),
                    ended,
                };
            }
        };
        if let Err(error) = self.set_permission_mode(target, process_config).await {
            return PlanHandoff::Ended {
                notice: Some(format!("could not execute approved plan: {error}")),
                ended: proposal,
            };
        }
        // A permission prompt carries no feedback, so the decision has none.
        let input = UserInput::text(IMPLEMENT_PLAN);
        PlanHandoff::Switched {
            mode: target,
            notice: format!(
                "plan approved; permission mode: {}",
                target.label().to_lowercase()
            ),
            implementation: Turn {
                display: input.clone(),
                model: input,
                calls: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
#[path = "plan_exit_tests.rs"]
mod tests;
