//! Terminal authentication uses the same login and credential store as the CLI.

use agent_client_protocol::schema::v1::{AuthMethod, AuthMethodTerminal, ClientCapabilities};
use rho_providers::provider::{self, ProviderAuthKind};

pub(super) fn methods(capabilities: &ClientCapabilities) -> Vec<AuthMethod> {
    if !capabilities.auth.terminal {
        return Vec::new();
    }
    provider::providers()
        .into_iter()
        .flat_map(|provider| provider.auth_modes())
        .filter(|mode| match mode.auth_kind {
            // Device-key setup only prints an authorization URL; there is no
            // completion to await, so it cannot promise terminal-auth success.
            ProviderAuthKind::None | ProviderAuthKind::OllamaDeviceKey { .. } => false,
            ProviderAuthKind::ApiKey { .. }
            | ProviderAuthKind::CodexOAuth { .. }
            | ProviderAuthKind::GithubCopilotDevice { .. }
            | ProviderAuthKind::XaiOAuth { .. }
            | ProviderAuthKind::BearerCredential { .. }
            | ProviderAuthKind::KimiOAuth { .. }
            | ProviderAuthKind::MetaOAuth { .. } => true,
        })
        .map(|mode| {
            AuthMethod::Terminal(
                AuthMethodTerminal::new(mode.id, mode.login_label)
                    .args(vec!["--login".into(), mode.id.into()]),
            )
        })
        .collect()
}
