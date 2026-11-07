//! Terminal classification and messages for a finished `rho run`.

use rho_protocol::TurnErrorKind;

use super::{AutomationExit, TerminalReason};
use crate::app::session_events::outcome::{authentication_message, failure_kind};

/// Human-readable terminal messages for one run's classified errors.
pub(super) struct TerminalMessages {
    authentication: String,
}

impl TerminalMessages {
    /// Precomputes the authentication message for the run's provider and auth.
    ///
    /// It names only those config identifiers and how to fix them, never the
    /// provider's error text, which may echo credentials.
    pub(super) fn for_selection(provider: &str, auth: &str) -> Self {
        Self {
            authentication: authentication_message(provider, auth),
        }
    }

    /// Builds the terminal message for a classified automation error.
    ///
    /// Reason codes stay stable machine labels. The message carries actionable
    /// detail; for authentication failures that is only the precomputed
    /// provider and auth hint, so credentials and token material never leave
    /// the process through stdout, stderr, or JSONL.
    pub(super) fn for_error(&self, reason: TerminalReason, error: &anyhow::Error) -> String {
        match reason {
            TerminalReason::Authentication => self.authentication.clone(),
            TerminalReason::ProviderError
            | TerminalReason::ToolHostError
            | TerminalReason::ConfigurationError
            | TerminalReason::OutputError
            | TerminalReason::OtherError
            | TerminalReason::Interrupted
            | TerminalReason::MaxSteps
            | TerminalReason::Timeout
            | TerminalReason::Completed => error.to_string(),
        }
    }
}

pub(super) fn classify_error(error: &anyhow::Error) -> (TerminalReason, u8) {
    if let Some(exit) = error.downcast_ref::<AutomationExit>() {
        return (exit.reason(), exit.exit_code());
    }
    match failure_kind(error) {
        TurnErrorKind::Authentication => (TerminalReason::Authentication, 1),
        TurnErrorKind::Provider => (TerminalReason::ProviderError, 1),
        TurnErrorKind::Tool => (TerminalReason::ToolHostError, 1),
        TurnErrorKind::Configuration => (TerminalReason::ConfigurationError, 2),
        // `TurnErrorKind` is non-exhaustive; a kind a newer protocol adds is
        // another error.
        _ => (TerminalReason::OtherError, 1),
    }
}
