//! Reports generated session titles to the host as `session_info_update`,
//! the update Zed names threads from and T3 Code records.
//!
//! A title lands seconds after the turn that asked for it, and the session
//! may have changed hands by then. Only the asking host reports it, and only
//! while that host is still published: once a `session/load` or
//! `session/resume` of the ID replaced it, or a close or delete retired it,
//! the title is not sent.
//!
//! The check and send hold the ID's install lock, which a load or resume
//! holds from before it reads the session until it publishes or fails, so no title
//! goes out while Rho loads the session; hosts such as T3 Code discard all
//! but replay then. A load that publishes retires the asking host. After a
//! failed load the title may still precede the load's error response, and
//! one sent just before Rho reads a load request can cross it on the wire.

use agent_client_protocol::schema::v1::{
    SessionId, SessionInfoUpdate, SessionNotification, SessionUpdate,
};

use super::{super::session_host::PendingTitle, AcpClientPort, RhoAcpAgent};
use crate::app::session_runtime::SessionHandle;

impl RhoAcpAgent {
    /// Generates the title `pending` asks for and sends it through `port`.
    pub(super) async fn report_title(&self, pending: PendingTitle, port: &dyn AcpClientPort) {
        let PendingTitle {
            session_id,
            asking,
            job,
        } = pending;
        if let Some(title) = job.generate().await {
            self.send_title(&session_id, &asking, title, port).await;
        }
    }

    /// Sends `title` for `session_id` if `asking`, the runtime handle of the
    /// host that asked for it, is still the one published under the ID.
    async fn send_title(
        &self,
        session_id: &SessionId,
        asking: &SessionHandle,
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
            .is_some_and(|live| live.handle.same_runtime(asking));
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
