//! Applies an approved plan at the end of the turn that proposed it, then
//! runs the implementation turn in the same `session/prompt`, as the TUI
//! queues it right after the proposal.

use agent_client_protocol::schema::v1::{
    CurrentModeUpdate, PromptResponse, SessionNotification, SessionUpdate, StopReason,
};
use rho_sdk::UserInput;

use super::{permission, send_notification, AcpClientPort, Config, EventMapper, SessionHost};
use crate::tools::plan_exit::{PlanExitResolution, ProposalTurn};

const IMPLEMENT_PLAN: &str = "Implement the approved plan now. Follow the plan and verify the changes; the host has switched this session's permission mode.";

impl SessionHost {
    /// Finishes a prompt whose turn may have answered `exit_plan_mode`,
    /// applying [`PlanExitDecision::resolve`]'s policy.
    ///
    /// [`PlanExitDecision::resolve`]: crate::tools::plan_exit::PlanExitDecision::resolve
    pub(super) async fn finish_plan_exit(
        &mut self,
        result: anyhow::Result<PromptResponse>,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let Some(decision) = self.built.tools.take_plan_exit_decision() else {
            return result;
        };
        // A cancel that lands after the proposal settled, such as during its
        // final usage update, still cancels the handoff. Checked right before
        // the switch commits; a later cancel stops only the implementation.
        let cancelled = self.prompt_gate.is_cancelled();
        let result = result.map(|mut response| {
            if cancelled {
                response.stop_reason = StopReason::Cancelled;
            }
            response
        });
        let turn = match &result {
            Ok(response) if response.stop_reason == StopReason::EndTurn => ProposalTurn::Completed,
            Ok(_) | Err(_) => ProposalTurn::Unfinished,
        };
        let target = match decision.resolve(turn, self.permission.mode) {
            PlanExitResolution::Apply(target) => target,
            PlanExitResolution::Keep(notice) => {
                self.plan_notice(notice, client).await;
                return result;
            }
        };
        if let Err(error) = self.set_permission_mode(target, process_config).await {
            self.plan_notice(
                &format!("could not execute approved plan: {}", error.message),
                client,
            )
            .await;
            return result;
        }
        // Hosts learn of agent-initiated switches only from these updates;
        // the config option form follows from the refresh.
        let mode = SessionNotification::new(
            self.acp_session_id.clone(),
            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(permission::mode_id(target))),
        );
        if let Err(error) = send_notification(client, mode).await {
            tracing::warn!(%error, "could not send ACP mode update");
        }
        self.refresh_advertisements(process_config, client).await;
        self.plan_notice(
            &format!(
                "plan approved; permission mode: {}",
                target.label().to_lowercase()
            ),
            client,
        )
        .await;
        // A permission prompt carries no feedback, so the decision has none.
        let input = UserInput::text(IMPLEMENT_PLAN);
        self.run_turn(&input, input.clone(), Vec::new(), client)
            .await
    }

    async fn plan_notice(&self, text: &str, client: &dyn AcpClientPort) {
        let notice = EventMapper::info_notice(&self.acp_session_id, self.host_updates, text);
        if let Err(error) = send_notification(client, notice).await {
            tracing::debug!(%error, "could not send ACP plan notice");
        }
    }
}
