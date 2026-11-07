//! How a failed `rho run` ends: its reason, exit code, and message.

use rho_protocol::{TurnError, TurnErrorKind};

use super::{AutomationExit, TerminalReason};
use crate::app::session_events::outcome::turn_error;

/// The exit of a run that failed with `error`, whose model runs on
/// `provider` with `auth`. An [`AutomationExit`] in the chain decides it;
/// otherwise the failure's kind does, with the same message the daemon
/// tells its clients: an authentication failure names only the provider,
/// the auth, and how to log in.
pub(super) fn failure(error: &anyhow::Error, provider: &str, auth: &str) -> AutomationExit {
    if let Some(exit) = error.downcast_ref::<AutomationExit>() {
        return exit.clone();
    }
    let TurnError { kind, message } = turn_error(error, provider, auth);
    let (reason, code) = match kind {
        TurnErrorKind::Authentication => (TerminalReason::Authentication, 1),
        TurnErrorKind::Provider => (TerminalReason::ProviderError, 1),
        TurnErrorKind::ToolHost => (TerminalReason::ToolHostError, 1),
        TurnErrorKind::Configuration => (TerminalReason::ConfigurationError, 2),
        // `TurnErrorKind` is non-exhaustive; a kind a newer protocol adds is
        // another error.
        _ => (TerminalReason::OtherError, 1),
    };
    AutomationExit::new(code, reason, message)
}
