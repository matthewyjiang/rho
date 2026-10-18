//! Applies plan approval only at the completed, durable turn boundary.

use super::{
    permission_mode::PermissionPersistence, App, Entry, InteractiveRuntime, QueuedPrompt,
    TurnOutcome,
};
use crate::tools::plan_exit::{PlanExitResolution, ProposalTurn};

impl App {
    pub(super) async fn finish_plan_exit(
        &mut self,
        outcome: &TurnOutcome,
        agent: &mut InteractiveRuntime,
    ) {
        let Some(decision) = agent.take_plan_exit_decision() else {
            return;
        };
        let turn = match outcome {
            TurnOutcome::Completed => ProposalTurn::Completed,
            TurnOutcome::Interrupted | TurnOutcome::Cancelled | TurnOutcome::Failed(_) => {
                ProposalTurn::Unfinished
            }
        };
        let target = match decision.resolve(turn, agent.permission_mode()) {
            PlanExitResolution::Apply(target) => target,
            PlanExitResolution::Keep(notice) => {
                self.insert_entry(&Entry::Notice(notice.into()));
                return;
            }
        };
        match self
            .apply_permission_mode(target, PermissionPersistence::UntilExit, agent)
            .await
        {
            Ok(()) => {
                // The approval names the implementation turn's mode; a cycle
                // queued during the proposal turn would otherwise overwrite it.
                self.pending_permission_mode = None;
                self.insert_entry(&Entry::Notice(format!(
                    "plan approved; permission mode: {} until Rho exits (not saved; next launch starts in your saved mode)",
                    target.label().to_lowercase(),
                )));
                self.pending.push_follow_up_front(QueuedPrompt::from(
                    "Implement the approved plan now. Follow the plan and verify the changes; the host has switched permission mode until Rho exits (not saved; the next launch starts in the saved mode).",
                ));
                self.pending_input_changed();
            }
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not execute approved plan: {error}",
            ))),
        }
    }
}
