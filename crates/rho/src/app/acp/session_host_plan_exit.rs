//! Reports the runtime's plan handoff (see [`PlanHandoff`]) at the end of the
//! turn that proposed the plan, and runs the implementation turn in the same
//! `session/prompt` when the runtime switched modes.

use agent_client_protocol::schema::v1::{
    CurrentModeUpdate, PromptResponse, SessionNotification, SessionUpdate, StopReason,
};

use super::{
    permission, prompt_response, send_notification, AcpClientPort, Config, EventMapper,
    SessionHost, Shown,
};
use crate::app::session_runtime::{PlanHandoff, TurnOutcome};

impl SessionHost {
    /// Shows `handoff`, which followed the turn that ended as `proposal`, and
    /// answers the prompt: with the proposal's end, or with the
    /// implementation turn's once the mode switched, and what the client
    /// missed of that turn.
    pub(super) async fn report_plan_handoff(
        &mut self,
        handoff: PlanHandoff,
        proposal: anyhow::Result<TurnOutcome>,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) -> Shown<PromptResponse> {
        match handoff {
            PlanHandoff::Kept { notice } => {
                self.plan_notice(&notice, client).await;
                Shown::delivered(proposal.map(prompt_response))
            }
            PlanHandoff::Cancelled { notice } => {
                self.plan_notice(&notice, client).await;
                Shown::delivered(proposal.map(|_| PromptResponse::new(StopReason::Cancelled)))
            }
            PlanHandoff::Switched {
                mode,
                notice,
                implementation,
            } => {
                // Hosts learn of agent-initiated switches only from these
                // updates; the config option form follows from the refresh.
                let update = SessionNotification::new(
                    self.acp_session_id.clone(),
                    SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(permission::mode_id(
                        mode,
                    ))),
                );
                if let Err(error) = send_notification(client, update).await {
                    tracing::warn!(%error, "could not send ACP mode update");
                }
                self.refresh_advertisements(process_config, client).await;
                self.plan_notice(&notice, client).await;
                let implemented = self
                    .forward_turn(implementation, process_config, client)
                    .await;
                Shown {
                    ended: implemented.ended.map(prompt_response),
                    missed_update: implemented.missed_update,
                }
            }
        }
    }

    async fn plan_notice(&self, text: &str, client: &dyn AcpClientPort) {
        let notice = EventMapper::info_notice(&self.acp_session_id, self.host_features, text);
        if let Err(error) = send_notification(client, notice).await {
            tracing::debug!(%error, "could not send ACP plan notice");
        }
    }
}
