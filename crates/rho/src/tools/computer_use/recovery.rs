//! Retain why an action revoked access, with one notification per grant.

use std::sync::Arc;

use rho_sdk::CancellationToken;

use super::{ComputerUseSession, State};

/// Exact audited Cua observation names whose calls cannot post desktop input.
/// Unknown names and every other allowed tool stay fail-closed.
///
/// File-output requests remain armed because they can write before failing.
/// MCP read-only hints, error text, and structured "no action" payloads are not
/// proof that a call had no effects.
pub(super) fn is_trusted_read_only(
    tool: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if arguments.contains_key("screenshot_out_file") {
        return false;
    }
    matches!(
        tool,
        "clipboard_read"
            | "get_accessibility_tree"
            | "get_browser_state"
            | "get_cursor_position"
            | "get_desktop_state"
            | "get_screen_size"
            | "get_window_state"
            | "list_apps"
            | "list_windows"
            | "verify_state"
            | "zoom"
    )
}

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
            State::Installing(_) | State::Connecting { .. } | State::Connected { .. } => None,
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
            State::Installing(_) | State::Connecting { .. } | State::Connected { .. } => None,
        }
    }
}

/// Dropped or failed calls have uncertain desktop effects unless the remote
/// name is an audited observation. A guard belongs to its original grant, so
/// late cleanup cannot revoke a newly authorized session.
pub(super) struct RevokeOnDrop {
    session: ComputerUseSession,
    grant: Arc<CancellationToken>,
    armed: bool,
    reason: String,
}

impl RevokeOnDrop {
    #[cfg(test)]
    pub(super) fn new(session: ComputerUseSession, grant: Arc<CancellationToken>) -> Self {
        Self::with_arm(session, grant, /*armed*/ true)
    }

    /// Action and unknown names stay armed. Trusted observation names retain
    /// the grant on failure, cancellation, or drop.
    pub(super) fn for_remote_call(
        session: ComputerUseSession,
        grant: Arc<CancellationToken>,
        tool: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self::with_arm(
            session,
            grant,
            /*armed*/ !is_trusted_read_only(tool, arguments),
        )
    }

    fn with_arm(session: ComputerUseSession, grant: Arc<CancellationToken>, armed: bool) -> Self {
        Self {
            session,
            grant,
            armed,
            reason: "computer action was interrupted".into(),
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }

    pub(super) fn record_error(&mut self, error: &rho_sdk::tool::ToolError) {
        if !self.armed || error.kind() == rho_sdk::tool::ToolErrorKind::Cancelled {
            return;
        }
        self.reason = format!("computer action failed: {error}");
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
