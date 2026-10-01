//! Credential acquisition for one provider auth mode.

use super::BearerCredentialAcquisition;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
    None,
    ApiKey {
        env_var: &'static str,
        account: &'static str,
        entry_label: &'static str,
        missing_message: &'static str,
    },
    CodexOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    GithubCopilotDevice {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    XaiOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    BearerCredential {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
        acquisition: BearerCredentialAcquisition,
    },
    KimiOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    /// Muse Code subscription. The device flow returns an identity token;
    /// inference uses a short-lived Model API key minted from that token.
    MetaOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    OllamaDeviceKey {
        missing_message: &'static str,
    },
}

impl ProviderAuthKind {
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::None | Self::OllamaDeviceKey { .. } => None,
            Self::ApiKey { env_var, .. }
            | Self::CodexOAuth { env_var, .. }
            | Self::GithubCopilotDevice { env_var, .. }
            | Self::XaiOAuth { env_var, .. }
            | Self::BearerCredential { env_var, .. }
            | Self::KimiOAuth { env_var, .. }
            | Self::MetaOAuth { env_var, .. } => Some(env_var),
        }
    }

    pub fn account(self) -> Option<&'static str> {
        match self {
            Self::None | Self::OllamaDeviceKey { .. } => None,
            Self::ApiKey { account, .. }
            | Self::CodexOAuth { account, .. }
            | Self::GithubCopilotDevice { account, .. }
            | Self::XaiOAuth { account, .. }
            | Self::BearerCredential { account, .. }
            | Self::KimiOAuth { account, .. }
            | Self::MetaOAuth { account, .. } => Some(account),
        }
    }

    pub(crate) fn has_browser_and_device_grants(self) -> bool {
        matches!(self, Self::CodexOAuth { .. } | Self::XaiOAuth { .. })
    }

    /// User-facing guidance when this auth kind has no usable credentials.
    pub fn missing_message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ApiKey {
                missing_message, ..
            }
            | Self::CodexOAuth {
                missing_message, ..
            }
            | Self::GithubCopilotDevice {
                missing_message, ..
            }
            | Self::XaiOAuth {
                missing_message, ..
            }
            | Self::BearerCredential {
                missing_message, ..
            }
            | Self::KimiOAuth {
                missing_message, ..
            }
            | Self::MetaOAuth {
                missing_message, ..
            }
            | Self::OllamaDeviceKey {
                missing_message, ..
            } => Some(missing_message),
        }
    }
}
