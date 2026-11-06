//! Shows the runtime's plan handoff (see [`PlanReport`]) in the prompt whose
//! turn answered the plan.

use agent_client_protocol::schema::v1::{CurrentModeUpdate, SessionNotification, SessionUpdate};

use super::{permission, send_notification, AcpClientPort, Config, EventMapper, SessionHost};
use crate::app::session_runtime::PlanReport;

impl SessionHost {
    /// Shows `report`: the notice, after a mode update and refreshed menus
    /// when the mode switched. Updates the client misses are logged; the
    /// prompt goes on either way.
    pub(super) async fn report_plan(
        &mut self,
        report: PlanReport,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) {
        let notice = match report {
            PlanReport::Kept { notice } => notice,
            PlanReport::Switched { mode, notice } => {
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
                notice
            }
        };
        let notice = EventMapper::info_notice(&self.acp_session_id, self.host_features, &notice);
        if let Err(error) = send_notification(client, notice).await {
            tracing::debug!(%error, "could not send ACP plan notice");
        }
    }
}
