//! Retain why an action revoked access, with one notification per grant.

use std::sync::Arc;

use rho_sdk::CancellationToken;

use super::{ComputerUseSession, State};

pub(super) struct Revocation {
    reason: String,
    reported: bool,
}

impl ComputerUseSession {
    pub(crate) fn revocation_reason(&self) -> Option<String> {
        match &*self.state() {
            State::Off { revocation, .. } | State::Closing { revocation, .. } => revocation
                .as_ref()
                .map(|revocation| revocation.reason.clone()),
            State::Connecting { .. } | State::Connected { .. } => None,
        }
    }

    pub(crate) fn take_revocation_notice(&self) -> Option<String> {
        match &mut *self.state() {
            State::Off { revocation, .. } | State::Closing { revocation, .. } => {
                let revocation = revocation.as_mut()?;
                if revocation.reported {
                    return None;
                }
                revocation.reported = true;
                Some(format!(
                    "computer access revoked: {}. The action may have partially completed; check the desktop before enabling access again with /computer on",
                    revocation.reason
                ))
            }
            State::Connecting { .. } | State::Connected { .. } => None,
        }
    }
}

/// Dropped or failed calls have uncertain desktop effects. A guard belongs to
/// its original grant, so late cleanup cannot revoke a newly authorized session.
pub(super) struct RevokeOnDrop {
    session: ComputerUseSession,
    grant: Arc<CancellationToken>,
    pub(super) armed: bool,
    reason: String,
}

impl RevokeOnDrop {
    pub(super) fn new(session: ComputerUseSession, grant: Arc<CancellationToken>) -> Self {
        Self {
            session,
            grant,
            armed: true,
            reason: "computer action was interrupted".into(),
        }
    }

    pub(super) fn record_error(&mut self, error: &rho_sdk::tool::ToolError) {
        if error.kind() != rho_sdk::tool::ToolErrorKind::Cancelled {
            self.reason = format!("computer action failed: {error}");
        }
    }
}

impl Drop for RevokeOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.session.revoke_grant(
                Some(&self.grant),
                Some(Revocation {
                    reason: self.reason.clone(),
                    reported: false,
                }),
            );
        }
    }
}
