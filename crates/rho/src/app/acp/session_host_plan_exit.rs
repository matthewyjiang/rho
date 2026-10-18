//! Applies an approved plan at the end of the turn that proposed it, then
//! runs the implementation turn in the same `session/prompt`, as the TUI
//! queues it right after the proposal.

use agent_client_protocol::schema::v1::{
    CurrentModeUpdate, PromptResponse, SessionNotification, SessionUpdate, StopReason,
};
use rho_sdk::UserInput;

use super::{permission, send_notification, AcpClientPort, Config, EventMapper, SessionHost};
use crate::permission::PermissionMode;

const IMPLEMENT_PLAN: &str = "Implement the approved plan now. Follow the plan and verify the changes; the host has switched this session's permission mode.";

impl SessionHost {
    /// Finishes a prompt whose turn may have answered `exit_plan_mode`.
    /// Approval applies only when the turn completed while the session is
    /// still in plan mode; otherwise the decision is dropped with a notice.
    pub(super) async fn finish_plan_exit(
        &mut self,
        result: anyhow::Result<PromptResponse>,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let Some(decision) = self.built.tools.take_plan_exit_decision() else {
            return result;
        };
        let completed = result
            .as_ref()
            .is_ok_and(|response| response.stop_reason == StopReason::EndTurn);
        if !completed {
            self.plan_notice("plan approval discarded; turn did not complete", client)
                .await;
            return result;
        }
        if decision.target == PermissionMode::Plan {
            self.plan_notice("plan not approved; permission mode stays plan", client)
                .await;
            return result;
        }
        if self.permission.mode != PermissionMode::Plan {
            self.plan_notice(
                "plan approval discarded; session is no longer in plan mode",
                client,
            )
            .await;
            return result;
        }
        if let Err(error) = self
            .set_permission_mode(decision.target, process_config)
            .await
        {
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
            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(permission::mode_id(
                decision.target,
            ))),
        );
        if let Err(error) = send_notification(client, mode).await {
            tracing::warn!(%error, "could not send ACP mode update");
        }
        self.refresh_advertisements(process_config, client).await;
        self.plan_notice(
            &format!(
                "plan approved; permission mode: {}",
                decision.target.label().to_lowercase()
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
