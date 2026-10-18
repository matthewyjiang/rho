//! Reports generated session titles to the host as `session_info_update`,
//! the update Zed names threads from and T3 Code records.
//!
//! A title lands seconds after the turn that asked for it, and the session
//! may have changed hands by then. Only the asking host reports it, and only
//! while that host is still published: once a `session/load` of the ID
//! replaced it, or a close or delete retired it, the title is not sent.
//!
//! The check and send hold the ID's install lock, which `load_session`
//! holds from before its replay until it publishes or fails, so no title
//! goes out while Rho loads the session; hosts such as T3 Code discard all
//! but replay then. A load that publishes retires the asking host. After a
//! failed load the title may still precede the load's error response, and
//! one sent just before Rho reads a load request can cross it on the wire.

use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    SessionId, SessionInfoUpdate, SessionNotification, SessionUpdate,
};

use super::{super::session_host::PendingTitle, AcpClientPort, PromptGate, RhoAcpAgent};

impl RhoAcpAgent {
    /// Generates the title `pending` asks for and sends it through `port`.
    pub(super) async fn report_title(&self, pending: PendingTitle, port: &dyn AcpClientPort) {
        let PendingTitle {
            session_id,
            asking_gate,
            job,
        } = pending;
        if let Some(title) = job.generate().await {
            self.send_title(&session_id, &asking_gate, title, port)
                .await;
        }
    }

    /// Sends `title` for `session_id` if `asking_gate`, the prompt gate of
    /// the host that asked for it, is still the one published under the ID.
    async fn send_title(
        &self,
        session_id: &SessionId,
        asking_gate: &Arc<PromptGate>,
        title: String,
        port: &dyn AcpClientPort,
    ) {
        let install = self.install_lock_for(session_id).await;
        let _install = install.lock().await;
        let published = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .is_some_and(|live| Arc::ptr_eq(&live.cancel, asking_gate));
        if !published {
            return;
        }
        let update = SessionNotification::new(
            session_id.clone(),
            SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)),
        );
        if let Err(error) = port.send_session_notification(update).await {
            tracing::debug!(%error, "could not send ACP session title");
        }
    }
}

#[cfg(test)]
#[path = "agent_titles_tests.rs"]
mod tests;
