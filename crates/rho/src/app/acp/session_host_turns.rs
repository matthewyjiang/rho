//! How a prompt's model turns show to the ACP client: the [`TurnView`] an
//! ACP host lends [`SessionRuntime::run_prompt_turn`]. Each turn runs beside
//! a forwarder (`session_host_forward.rs`); the plan handoff between turns
//! shows as a mode update, refreshed menus, and a notice.

use std::{future::Future, sync::atomic::AtomicU64};

use agent_client_protocol::schema::v1::{
    CurrentModeUpdate, PlanEntry, SessionId, SessionNotification, SessionUpdate,
};
use tokio::sync::mpsc;

use super::{
    advertisements::Advertisements, forward, permission, send_notification, session_signals,
    AcpClientPort, Config, EventMapper, HostFeatures, PromptAcceptance,
};
use crate::app::session_runtime::{PlanReport, SessionRuntime, TurnIo, TurnView};

/// How one prompt's model turns show to `client`: the host state a turn's
/// forwarder reads and writes back, borrowed field by field so the runtime
/// stays free to run the turns.
pub(super) struct AcpTurns<'a> {
    pub(super) session_id: &'a SessionId,
    pub(super) client: &'a dyn AcpClientPort,
    pub(super) features: HostFeatures,
    pub(super) placeholders: &'a AtomicU64,
    pub(super) acceptance: &'a mut PromptAcceptance,
    /// The todo checklist the client last received as an ACP `plan`.
    pub(super) sent_plan: &'a mut Vec<PlanEntry>,
    pub(super) advertisements: &'a mut Advertisements,
    /// The first update of the prompt's turns the client missed, if any.
    pub(super) missed_update: Option<String>,
}

impl<'a> TurnView for AcpTurns<'a> {
    /// Shows the turn through a forwarder (`session_host_forward.rs`).
    fn open_turn<'v>(
        &'v mut self,
        runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v, 'a>) {
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (control_sender, control) = mpsc::unbounded_channel();
        let forwarder = forward::Forwarder {
            session_id: self.session_id,
            client: self.client,
            features: self.features,
            mapper: EventMapper::new(
                self.features,
                runtime.cwd().to_path_buf(),
                runtime.tools().mcp_report().tool_names(),
            ),
            // See `Forwarded::sent_plan` for when the checklist comes back.
            signals: session_signals(runtime, self.sent_plan),
            placeholders: self.placeholders,
            acceptance: &mut *self.acceptance,
        };
        let sent_plan = &mut *self.sent_plan;
        let missed_update = &mut self.missed_update;
        let showing = async move {
            let forwarded = forwarder.forward(event_receiver, control_sender).await;
            *sent_plan = forwarded.sent_plan.unwrap_or_default();
            // The first turn's miss came first, so it is the one reported.
            if missed_update.is_none() {
                *missed_update = forwarded.failure;
            }
        };
        (TurnIo { events, control }, showing)
    }

    /// Shows `report`: the notice, after a mode update and refreshed menus
    /// when the mode switched. Updates the client misses are logged; the
    /// prompt goes on either way.
    fn report_plan(
        &mut self,
        runtime: &SessionRuntime,
        process_config: &Config,
        report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        let (session_id, client, features) = (self.session_id, self.client, self.features);
        let (notice, switched) = match report {
            PlanReport::Kept { notice } => (notice, None),
            PlanReport::Switched { mode, notice } => {
                let refresh = self.advertisements.refresh(
                    session_id,
                    runtime,
                    features.toggles,
                    process_config,
                    client,
                );
                (notice, Some((mode, refresh)))
            }
        };
        async move {
            if let Some((mode, refresh)) = switched {
                // Hosts learn of agent-initiated switches only from these
                // updates; the config option form follows from the refresh.
                let update = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(permission::mode_id(
                        mode,
                    ))),
                );
                if let Err(error) = send_notification(client, update).await {
                    tracing::warn!(%error, "could not send ACP mode update");
                }
                refresh.await;
            }
            let notice = EventMapper::info_notice(session_id, features, &notice);
            if let Err(error) = send_notification(client, notice).await {
                tracing::debug!(%error, "could not send ACP plan notice");
            }
        }
    }
}
