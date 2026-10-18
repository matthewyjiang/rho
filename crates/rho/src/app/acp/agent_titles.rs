//! Reports generated session titles to the host as `session_info_update`,
//! the update Zed names threads from and T3 Code records.
//!
//! A title lands seconds after the turn that asked for it, and the session
//! may have changed hands by then. Only the asking host reports it, and only
//! while that host is still published: once a `session/load` of the ID
//! replaced it, or a close or delete retired it, the title stays saved for
//! `session/list` but is not sent.
//!
//! The check and send hold the ID's install lock, which a load holds from
//! before its replay until it publishes. So no title goes out while a load
//! of its session is in flight, when hosts such as T3 Code discard all but
//! replay, and a load that publishes leaves the asking host retired.

use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    SessionId, SessionInfoUpdate, SessionNotification, SessionUpdate,
};

use super::{super::session_host::PendingTitle, AcpClientPort, PromptGate, RhoAcpAgent};

impl RhoAcpAgent {
    /// Generates the title `pending` asks for and sends it through `port`.
    pub(super) async fn report_title(&self, pending: PendingTitle, port: &dyn AcpClientPort) {
        let session_id = pending.session_id.clone();
        let asking_host = Arc::clone(&pending.host);
        if let Some(title) = pending.generate().await {
            self.send_title(&session_id, &asking_host, title, port)
                .await;
        }
    }

    /// Sends `title` for `session_id` if `asking_host`, the prompt gate of
    /// the host that asked for it, is still the one published under the ID.
    async fn send_title(
        &self,
        session_id: &SessionId,
        asking_host: &Arc<PromptGate>,
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
            .is_some_and(|live| Arc::ptr_eq(&live.cancel, asking_host));
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
