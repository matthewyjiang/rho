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

/// What became of the plan a prompt's turn answered. Each carries a
/// `notice` for the host to show the user.
pub(crate) enum PlanHandoff {
    /// The session keeps its mode: the plan was not approved, the turn did
    /// not complete, or the switch failed.
    Kept { notice: String },
    /// A cancel arrived after the proposing turn ended: the approval is
    /// discarded, the mode kept, and the prompt ends cancelled.
    Cancelled { notice: String },
    /// The session is now in `mode`. Report the switch, then run
    /// `implementation` as the prompt's next turn.
    Switched {
        mode: PermissionMode,
        notice: String,
        implementation: Turn,
    },
}

impl SessionRuntime {
    /// Applies the answer to `exit_plan_mode` that `prompt`'s turn ending
    /// as `proposal` recorded, if any. Call once the host has shown that
    /// turn.
    /// `proposal` is how the turn itself ended: an update the host failed to
    /// show does not change it, since the turn was saved either way. A cancel that
    /// lands before the switch commits, such as during the host's last
    /// update of the proposal, still cancels the handoff; a later one stops
    /// only the implementation turn.
    pub(crate) async fn hand_off_plan(
        &mut self,
        prompt: &PromptScope,
        proposal: &anyhow::Result<TurnOutcome>,
        process_config: &Config,
    ) -> Option<PlanHandoff> {
        let decision = self.built.tools.take_plan_exit_decision()?;
        let cancelled = prompt.is_cancelled();
        let turn = match proposal {
            Ok(outcome) if outcome.answered() && !cancelled => ProposalTurn::Completed,
            Ok(_) | Err(_) => ProposalTurn::Unfinished,
        };
        let target = match decision.resolve(turn, self.permission.mode) {
            PlanExitResolution::Apply(target) => target,
            PlanExitResolution::Keep(notice) if cancelled => {
                return Some(PlanHandoff::Cancelled {
                    notice: notice.to_owned(),
                })
            }
            PlanExitResolution::Keep(notice) => {
                return Some(PlanHandoff::Kept {
                    notice: notice.to_owned(),
                })
            }
        };
        if let Err(error) = self.set_permission_mode(target, process_config).await {
            return Some(PlanHandoff::Kept {
                notice: format!("could not execute approved plan: {error}"),
            });
        }
        // A permission prompt carries no feedback, so the decision has none.
        let input = UserInput::text(IMPLEMENT_PLAN);
        Some(PlanHandoff::Switched {
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
        })
    }
}
