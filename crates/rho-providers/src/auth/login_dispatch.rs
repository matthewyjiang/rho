use std::{future::Future, pin::Pin};

use crate::{
    auth::{codex_oauth, github_copilot_device, kimi_oauth, xai_oauth},
    credentials::{
        self, CodexTokens, CredentialResult, CredentialStore, GitHubCopilotTokens, KimiTokens,
        XaiTokens,
    },
    provider::{self, ProviderAuthKind},
};

pub type AuthenticationFuture = Pin<
    Box<dyn Future<Output = Result<CompletedAuthentication, AuthenticationError>> + Send + 'static>,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationMethod {
    None,
    ApiKey { entry_label: &'static str },
    OAuth { provider_label: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthMode {
    Browser,
    Device,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthUserAction {
    BrowserOpened,
    DeviceCode {
        verification_uri: String,
        user_code: String,
        verification_uri_complete: Option<String>,
    },
}

pub struct OAuthLogin {
    pub provider_label: &'static str,
    pub user_action: OAuthUserAction,
    pub completion: AuthenticationFuture,
}

impl std::fmt::Debug for OAuthLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthLogin")
            .field("provider_label", &self.provider_label)
            .field("user_action", &self.user_action)
            .field("completion", &"<authentication future>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("unsupported login provider '{0}'")]
    UnsupportedProvider(String),
    #[error("provider '{0}' does not use OAuth")]
    NotOAuth(String),
    #[error("{0}")]
    Flow(String),
}

pub struct CompletedAuthentication {
    credentials: OAuthCredentials,
}

impl CompletedAuthentication {
    pub fn save(self, store: &dyn CredentialStore) -> CredentialResult<()> {
        match self.credentials {
            OAuthCredentials::Codex(tokens) => credentials::save_codex_tokens(store, &tokens),
            OAuthCredentials::GithubCopilot(tokens) => {
                credentials::save_github_copilot_tokens(store, &tokens)
            }
            OAuthCredentials::Kimi(tokens) => credentials::save_kimi_tokens(store, &tokens),
            OAuthCredentials::Xai(tokens) => credentials::save_xai_tokens(store, &tokens),
        }
    }
}

impl std::fmt::Debug for CompletedAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CompletedAuthentication([REDACTED])")
    }
}

enum OAuthCredentials {
    Codex(CodexTokens),
    GithubCopilot(GitHubCopilotTokens),
    Kimi(KimiTokens),
    Xai(XaiTokens),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderAuthentication;

impl ProviderAuthentication {
    pub fn method(provider_name: &str) -> Result<AuthenticationMethod, AuthenticationError> {
        let descriptor = provider::provider_descriptor(provider_name)
            .ok_or_else(|| AuthenticationError::UnsupportedProvider(provider_name.into()))?;
        Ok(match descriptor.auth_kind {
            ProviderAuthKind::None => AuthenticationMethod::None,
            ProviderAuthKind::ApiKey { entry_label, .. } => {
                AuthenticationMethod::ApiKey { entry_label }
            }
            ProviderAuthKind::CodexOAuth { .. } => AuthenticationMethod::OAuth {
                provider_label: "Codex",
            },
            ProviderAuthKind::GithubCopilotDevice { .. } => AuthenticationMethod::OAuth {
                provider_label: "GitHub Copilot",
            },
            ProviderAuthKind::KimiOAuth { .. } => AuthenticationMethod::OAuth {
                provider_label: "Kimi",
            },
            ProviderAuthKind::XaiOAuth { .. } => AuthenticationMethod::OAuth {
                provider_label: "xAI",
            },
        })
    }

    pub async fn start_oauth(
        provider_name: &str,
        mode: OAuthMode,
    ) -> Result<OAuthLogin, AuthenticationError> {
        let descriptor = provider::provider_descriptor(provider_name)
            .ok_or_else(|| AuthenticationError::UnsupportedProvider(provider_name.into()))?;
        match descriptor.auth_kind {
            ProviderAuthKind::None | ProviderAuthKind::ApiKey { .. } => {
                Err(AuthenticationError::NotOAuth(provider_name.into()))
            }
            ProviderAuthKind::CodexOAuth { .. } => start_codex(mode).await,
            ProviderAuthKind::GithubCopilotDevice { .. } => start_github_copilot().await,
            ProviderAuthKind::KimiOAuth { .. } => start_kimi().await,
            ProviderAuthKind::XaiOAuth { .. } => start_xai(mode).await,
        }
    }

    pub fn save_api_key(
        store: &dyn CredentialStore,
        provider_name: &str,
        key: &str,
    ) -> CredentialResult<()> {
        credentials::save_provider_api_key(store, provider_name, key)
    }

    pub fn delete_credentials(
        store: &dyn CredentialStore,
        provider_name: &str,
    ) -> CredentialResult<bool> {
        credentials::delete_provider_credentials(store, provider_name)
    }

    pub fn has_credentials(
        store: &dyn CredentialStore,
        provider_name: &str,
    ) -> CredentialResult<bool> {
        credentials::provider_has_credentials(store, provider_name)
    }

    pub fn has_stored_credentials(
        store: &dyn CredentialStore,
        provider_name: &str,
    ) -> CredentialResult<bool> {
        credentials::provider_has_stored_credentials(store, provider_name)
    }

    pub fn has_environment_override(provider_name: &str) -> bool {
        credentials::provider_has_env_override(provider_name)
    }
}

async fn start_codex(mode: OAuthMode) -> Result<OAuthLogin, AuthenticationError> {
    if mode == OAuthMode::Browser {
        return Ok(OAuthLogin {
            provider_label: "Codex",
            user_action: OAuthUserAction::BrowserOpened,
            completion: Box::pin(async {
                codex_oauth::run_codex_oauth_flow()
                    .await
                    .map(|tokens| CompletedAuthentication {
                        credentials: OAuthCredentials::Codex(tokens),
                    })
                    .map_err(flow_error)
            }),
        });
    }

    let login = codex_oauth::start_codex_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = OAuthUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: None,
    };
    Ok(OAuthLogin {
        provider_label: "Codex",
        user_action,
        completion: Box::pin(async move {
            codex_oauth::complete_codex_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: OAuthCredentials::Codex(tokens),
                })
                .map_err(flow_error)
        }),
    })
}

async fn start_github_copilot() -> Result<OAuthLogin, AuthenticationError> {
    let login = github_copilot_device::start_github_copilot_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = OAuthUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: login.verification_uri_complete.clone(),
    };
    Ok(OAuthLogin {
        provider_label: "GitHub Copilot",
        user_action,
        completion: Box::pin(async move {
            github_copilot_device::complete_github_copilot_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: OAuthCredentials::GithubCopilot(tokens),
                })
                .map_err(flow_error)
        }),
    })
}

async fn start_kimi() -> Result<OAuthLogin, AuthenticationError> {
    let login = kimi_oauth::start_kimi_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = OAuthUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: login.verification_uri_complete.clone(),
    };
    Ok(OAuthLogin {
        provider_label: "Kimi",
        user_action,
        completion: Box::pin(async move {
            kimi_oauth::complete_kimi_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: OAuthCredentials::Kimi(tokens),
                })
                .map_err(flow_error)
        }),
    })
}

async fn start_xai(mode: OAuthMode) -> Result<OAuthLogin, AuthenticationError> {
    if mode == OAuthMode::Browser {
        return Ok(OAuthLogin {
            provider_label: "xAI",
            user_action: OAuthUserAction::BrowserOpened,
            completion: Box::pin(async {
                xai_oauth::run_xai_oauth_flow()
                    .await
                    .map(|tokens| CompletedAuthentication {
                        credentials: OAuthCredentials::Xai(tokens),
                    })
                    .map_err(flow_error)
            }),
        });
    }

    let login = xai_oauth::start_xai_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = OAuthUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: login.verification_uri_complete.clone(),
    };
    Ok(OAuthLogin {
        provider_label: "xAI",
        user_action,
        completion: Box::pin(async move {
            xai_oauth::complete_xai_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: OAuthCredentials::Xai(tokens),
                })
                .map_err(flow_error)
        }),
    })
}

fn flow_error(error: impl std::fmt::Display) -> AuthenticationError {
    AuthenticationError::Flow(error.to_string())
}

#[cfg(test)]
#[path = "login_dispatch_tests.rs"]
mod tests;
