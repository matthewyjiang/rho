//! Optional session updates the host opted into at `initialize`.
//!
//! ACP lets an agent send `compaction_update` and `notice` only to clients
//! that advertise `clientCapabilities.session.compaction` and `.notices`.
//! Other hosts keep the baseline updates: `/compact` replies with an agent
//! message, automatic compaction shows only in `usage_update`, and notices
//! are agent messages.

use std::sync::OnceLock;

use agent_client_protocol::schema::v1::ClientCapabilities;

/// Session updates beyond the ACP baseline that the connected host accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct HostUpdates {
    /// Compaction lifecycle cards (`compaction_update`).
    pub(super) compaction: bool,
    /// Live, dismissible user notices (`notice`).
    pub(super) notices: bool,
}

impl HostUpdates {
    pub(super) fn from_capabilities(capabilities: &ClientCapabilities) -> Self {
        let session = capabilities.session.as_ref();
        Self {
            compaction: session.is_some_and(|session| session.compaction.is_some()),
            notices: session.is_some_and(|session| session.notices.is_some()),
        }
    }
}

/// The host's choice, recorded by `initialize` and read when the first
/// session starts. A host that skips `initialize` gets the baseline.
#[derive(Default)]
pub(super) struct NegotiatedUpdates(OnceLock<HostUpdates>);

impl NegotiatedUpdates {
    /// Keeps the first `initialize`; ACP negotiates capabilities once per
    /// connection.
    pub(super) fn record(&self, capabilities: &ClientCapabilities) {
        let _ = self.0.set(HostUpdates::from_capabilities(capabilities));
    }

    pub(super) fn get(&self) -> HostUpdates {
        self.0.get().copied().unwrap_or_default()
    }
}
