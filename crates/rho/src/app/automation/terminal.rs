//! Terminal classification and messages for a finished `rho run`.

use super::{AutomationExit, AutomationInterrupted, TerminalReason};

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
        let alternatives = rho_providers::provider::provider_descriptor(provider)
            .into_iter()
            .flat_map(|descriptor| descriptor.auth_modes())
            .filter(|mode| {
                mode.id != auth
                    && !matches!(
                        mode.auth_kind,
                        rho_providers::provider::ProviderAuthKind::None
                    )
            })
            .map(|mode| format!("--auth {}", mode.id))
            .collect::<Vec<_>>();
        let mut authentication =
            format!("authentication failed for {provider} (auth: {auth}); run /login {provider}");
        if !alternatives.is_empty() {
            authentication.push_str(" or pass ");
            authentication.push_str(&alternatives.join(" or "));
        }
        Self { authentication }
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
    if let Some(interrupted) = error.downcast_ref::<AutomationInterrupted>() {
        return (TerminalReason::Interrupted, interrupted.exit_code());
    }
    if let Some(exit) = error.downcast_ref::<AutomationExit>() {
        return (exit.reason(), exit.exit_code());
    }
    for cause in error.chain() {
        if let Some(error) = cause.downcast_ref::<rho_sdk::Error>() {
            return match error {
                rho_sdk::Error::Authentication { .. } => (TerminalReason::Authentication, 1),
                rho_sdk::Error::Provider(provider)
                    if provider.kind() == rho_sdk::ProviderErrorKind::Authentication =>
                {
                    (TerminalReason::Authentication, 1)
                }
                rho_sdk::Error::Provider(_) => (TerminalReason::ProviderError, 1),
                rho_sdk::Error::Tool(_) => (TerminalReason::ToolHostError, 1),
                rho_sdk::Error::InvalidConfiguration { .. } => {
                    (TerminalReason::ConfigurationError, 2)
                }
                _ => (TerminalReason::OtherError, 1),
            };
        }
        if let Some(error) = cause.downcast_ref::<rho_providers::model::ModelError>() {
            use rho_providers::model::ModelError;
            return match error {
                ModelError::MissingCredentials(_) | ModelError::Credentials(_) => {
                    (TerminalReason::Authentication, 1)
                }
                ModelError::UnsupportedReasoning { .. } | ModelError::UnsupportedProvider(_) => {
                    (TerminalReason::ConfigurationError, 2)
                }
                _ => (TerminalReason::ProviderError, 1),
            };
        }
    }
    (TerminalReason::OtherError, 1)
}
