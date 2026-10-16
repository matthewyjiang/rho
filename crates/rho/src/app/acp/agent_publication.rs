//! Hold a new/load host until its initial state is on the wire. In particular,
//! hosts may discard live updates while load and old-runtime teardown run.

use std::{fmt, sync::Arc};

use agent_client_protocol::schema::v1::SessionId;

use super::{HostGuard, LiveSession};

pub(in crate::app::acp) struct PublicationGuard {
    host: HostGuard,
    sent: bool,
}

impl PublicationGuard {
    /// Acquire before publishing this freshly built, occupied session slot.
    pub(super) async fn new(live: &LiveSession, session_id: &SessionId) -> Self {
        let held = Arc::clone(&live.host).lock_owned().await;
        Self {
            host: HostGuard::occupied(held, session_id)
                .expect("a freshly built session has an occupied host slot"),
            sent: false,
        }
    }

    /// The response and initial commands were both accepted by the transport.
    pub(in crate::app::acp) fn sent(mut self) {
        self.sent = true;
    }
}

impl Drop for PublicationGuard {
    fn drop(&mut self) {
        if !self.sent {
            // A later prompt/settings request must retry the full state rather
            // than treating an initial payload we failed to send as delivered.
            self.host.forget_advertisements();
        }
    }
}

impl fmt::Debug for PublicationGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicationGuard")
            .field("sent", &self.sent)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "agent_publication_tests.rs"]
mod tests;
