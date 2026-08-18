use std::{future::Future, pin::Pin};

use crate::{
    auth::{
        codex_oauth, github_copilot_device, kimi_oauth, ollama_device, openrouter_oauth, xai_oauth,
    },
    credentials::{
        self, CodexTokens, CredentialResult, CredentialStore, GitHubCopilotTokens, KimiTokens,
        XaiTokens,
    },
    provider::{
        self, BearerCredentialAcquisition, BrowserOAuthFlow, ProviderAuthKind,
        ResolvedProviderProfile,
    },
};

pub type AuthenticationFuture = Pin<
    Box<dyn Future<Output = Result<CompletedAuthentication, AuthenticationError>> + Send + 'static>,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationMethod {
    None,
    ApiKey { entry_label: &'static str },
    Interactive { provider_label: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractiveLoginMode {
    Browser,
    Device,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InteractiveUserAction {
    BrowserOpened,
    /// Show a URL to open manually (no device code). Used by Ollama device-key connect.
    OpenUrl {
        url: String,
        instruction: String,
    },
    DeviceCode {
        verification_uri: String,
        user_code: String,
        verification_uri_complete: Option<String>,
    },
}

pub enum InteractiveLoginCompletion {
    Confirm(AuthenticationFuture),
    /// The external setup has started, but the provider cannot reliably confirm completion.
    Unconfirmed {
        instruction: &'static str,
    },
}

impl std::fmt::Debug for InteractiveLoginCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Confirm(_) => formatter.write_str("Confirm(<authentication future>)"),
            Self::Unconfirmed { instruction } => formatter
                .debug_struct("Unconfirmed")
                .field("instruction", instruction)
                .finish(),
        }
    }
}

pub struct InteractiveLogin {
    pub provider_label: &'static str,
    pub user_action: InteractiveUserAction,
    pub completion: InteractiveLoginCompletion,
}

impl std::fmt::Debug for InteractiveLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InteractiveLogin")
            .field("provider_label", &self.provider_label)
            .field("user_action", &self.user_action)
            .field("completion", &self.completion)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("unsupported login provider '{0}'")]
    UnsupportedProvider(String),
    #[error(
        "provider '{provider}' has multiple auth modes; specify one of: {auth_ids}",
        auth_ids = .auth_ids.join(", ")
    )]
    AmbiguousProvider {
        provider: String,
        auth_ids: Vec<&'static str>,
    },
    #[error("provider '{0}' does not use interactive login")]
    NotInteractive(String),
    #[error("{0}")]
    Flow(String),
}

pub struct CompletedAuthentication {
    credentials: LoginCredentials,
}

impl CompletedAuthentication {
    pub fn save(self, store: &dyn CredentialStore) -> CredentialResult<()> {
        match self.credentials {
            LoginCredentials::Codex(tokens) => credentials::save_codex_tokens(store, &tokens),
            LoginCredentials::GithubCopilot(tokens) => {
                credentials::save_github_copilot_tokens(store, &tokens)
            }
            LoginCredentials::Kimi(tokens) => credentials::save_kimi_tokens(store, &tokens),
            LoginCredentials::OpenRouter(key) => {
                credentials::save_openrouter_oauth_key(store, &key)
            }
            LoginCredentials::Xai(tokens) => credentials::save_xai_tokens(store, &tokens),
        }
    }
}

impl std::fmt::Debug for CompletedAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CompletedAuthentication([REDACTED])")
    }
}

enum LoginCredentials {
    Codex(CodexTokens),
    GithubCopilot(GitHubCopilotTokens),
    Kimi(KimiTokens),
    OpenRouter(String),
    Xai(XaiTokens),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderAuthentication;

impl ProviderAuthentication {
    pub fn method(provider_or_auth: &str) -> Result<AuthenticationMethod, AuthenticationError> {
        let profile = resolve_login_profile(provider_or_auth)?;
        Ok(match profile.auth_kind() {
            ProviderAuthKind::None => AuthenticationMethod::None,
            ProviderAuthKind::ApiKey { entry_label, .. } => {
                AuthenticationMethod::ApiKey { entry_label }
            }
            ProviderAuthKind::CodexOAuth { .. } => AuthenticationMethod::Interactive {
                provider_label: "Codex",
            },
            ProviderAuthKind::GithubCopilotDevice { .. } => AuthenticationMethod::Interactive {
                provider_label: "GitHub Copilot",
            },
            ProviderAuthKind::KimiOAuth { .. } => AuthenticationMethod::Interactive {
                provider_label: "Kimi",
            },
            ProviderAuthKind::XaiOAuth { .. } => AuthenticationMethod::Interactive {
                provider_label: "xAI",
            },
            ProviderAuthKind::OllamaDeviceKey { .. } => AuthenticationMethod::Interactive {
                provider_label: "Ollama Cloud",
            },
            ProviderAuthKind::BearerCredential { acquisition, .. } => match acquisition {
                BearerCredentialAcquisition::BrowserOAuth(flow) => {
                    AuthenticationMethod::Interactive {
                        provider_label: flow.provider_label(),
                    }
                }
            },
        })
    }

    pub fn supports_device_login(provider_or_auth: &str) -> bool {
        resolve_login_profile(provider_or_auth).is_ok_and(|profile| {
            matches!(
                profile.auth_kind(),
                ProviderAuthKind::CodexOAuth { .. }
                    | ProviderAuthKind::GithubCopilotDevice { .. }
                    | ProviderAuthKind::KimiOAuth { .. }
                    | ProviderAuthKind::XaiOAuth { .. }
                    | ProviderAuthKind::OllamaDeviceKey { .. }
            )
        })
    }

    pub async fn start_interactive_login(
        provider_or_auth: &str,
        mode: InteractiveLoginMode,
    ) -> Result<InteractiveLogin, AuthenticationError> {
        let profile = resolve_login_profile(provider_or_auth)?;
        match profile.auth_kind() {
            ProviderAuthKind::None | ProviderAuthKind::ApiKey { .. } => {
                Err(AuthenticationError::NotInteractive(provider_or_auth.into()))
            }
            ProviderAuthKind::CodexOAuth { .. } => start_codex(mode).await,
            ProviderAuthKind::GithubCopilotDevice { .. } => start_github_copilot().await,
            ProviderAuthKind::KimiOAuth { .. } => start_kimi().await,
            ProviderAuthKind::XaiOAuth { .. } => start_xai(mode).await,
            ProviderAuthKind::OllamaDeviceKey { .. } => start_ollama_device(mode).await,
            ProviderAuthKind::BearerCredential { acquisition, .. } => match acquisition {
                BearerCredentialAcquisition::BrowserOAuth(BrowserOAuthFlow::OpenRouter) => {
                    start_openrouter(mode).await
                }
            },
        }
    }

    pub fn save_api_key(
        store: &dyn CredentialStore,
        provider_or_auth: &str,
        key: &str,
    ) -> CredentialResult<()> {
        credentials::save_provider_api_key(store, provider_or_auth, key)
    }

    /// Deletes credentials for an auth profile id, or every mode on a provider name.
    ///
    /// Auth profile ids are preferred. Provider names delete all modes for that
    /// provider (used by doctor/logout at provider granularity).
    pub fn delete_credentials(
        store: &dyn CredentialStore,
        provider_or_auth: &str,
    ) -> CredentialResult<bool> {
        if provider::resolve_auth_mode(provider_or_auth).is_some() {
            credentials::delete_auth_credentials(store, provider_or_auth)
        } else {
            credentials::delete_provider_credentials(store, provider_or_auth)
        }
    }

    /// True when the auth profile or any mode on the provider has credentials.
    ///
    /// Auth profile ids check one mode. Provider names check any mode (doctor).
    pub fn has_credentials(
        store: &dyn CredentialStore,
        provider_or_auth: &str,
    ) -> CredentialResult<bool> {
        if provider::resolve_auth_mode(provider_or_auth).is_some() {
            credentials::auth_has_credentials(store, provider_or_auth)
        } else if provider::provider_descriptor(provider_or_auth).is_some() {
            credentials::provider_has_credentials(store, provider_or_auth)
        } else {
            Ok(false)
        }
    }

    pub fn has_stored_credentials(
        store: &dyn CredentialStore,
        provider_or_auth: &str,
    ) -> CredentialResult<bool> {
        if provider::resolve_auth_mode(provider_or_auth).is_some() {
            credentials::auth_has_stored_credentials(store, provider_or_auth)
        } else if provider::provider_descriptor(provider_or_auth).is_some() {
            credentials::provider_has_stored_credentials(store, provider_or_auth)
        } else {
            Ok(false)
        }
    }

    pub fn has_environment_override(provider_or_auth: &str) -> bool {
        if provider::resolve_auth_mode(provider_or_auth).is_some() {
            credentials::auth_has_env_override(provider_or_auth)
        } else {
            credentials::provider_has_env_override(provider_or_auth)
        }
    }
}

fn resolve_login_profile(
    provider_or_auth: &str,
) -> Result<ResolvedProviderProfile, AuthenticationError> {
    if let Some((provider, mode)) = provider::resolve_auth_mode(provider_or_auth) {
        return Ok(ResolvedProviderProfile {
            provider,
            auth: mode,
        });
    }
    let descriptor = provider::provider_descriptor(provider_or_auth)
        .ok_or_else(|| AuthenticationError::UnsupportedProvider(provider_or_auth.into()))?;
    match descriptor.auth_modes {
        [only] => Ok(ResolvedProviderProfile {
            provider: descriptor,
            auth: *only,
        }),
        modes => Err(AuthenticationError::AmbiguousProvider {
            provider: provider_or_auth.into(),
            auth_ids: modes
                .iter()
                .map(|mode| mode.id)
                .filter(|id| *id != "none")
                .collect(),
        }),
    }
}

async fn start_codex(mode: InteractiveLoginMode) -> Result<InteractiveLogin, AuthenticationError> {
    if mode == InteractiveLoginMode::Browser {
        return Ok(InteractiveLogin {
            provider_label: "Codex",
            user_action: InteractiveUserAction::BrowserOpened,
            completion: InteractiveLoginCompletion::Confirm(Box::pin(async {
                codex_oauth::run_codex_oauth_flow()
                    .await
                    .map(|tokens| CompletedAuthentication {
                        credentials: LoginCredentials::Codex(tokens),
                    })
                    .map_err(flow_error)
            })),
        });
    }

    let login = codex_oauth::start_codex_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = InteractiveUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: None,
    };
    Ok(InteractiveLogin {
        provider_label: "Codex",
        user_action,
        completion: InteractiveLoginCompletion::Confirm(Box::pin(async move {
            codex_oauth::complete_codex_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: LoginCredentials::Codex(tokens),
                })
                .map_err(flow_error)
        })),
    })
}

async fn start_github_copilot() -> Result<InteractiveLogin, AuthenticationError> {
    let login = github_copilot_device::start_github_copilot_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = InteractiveUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: login.verification_uri_complete.clone(),
    };
    Ok(InteractiveLogin {
        provider_label: "GitHub Copilot",
        user_action,
        completion: InteractiveLoginCompletion::Confirm(Box::pin(async move {
            github_copilot_device::complete_github_copilot_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: LoginCredentials::GithubCopilot(tokens),
                })
                .map_err(flow_error)
        })),
    })
}

async fn start_kimi() -> Result<InteractiveLogin, AuthenticationError> {
    let login = kimi_oauth::start_kimi_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = InteractiveUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: login.verification_uri_complete.clone(),
    };
    Ok(InteractiveLogin {
        provider_label: "Kimi",
        user_action,
        completion: InteractiveLoginCompletion::Confirm(Box::pin(async move {
            kimi_oauth::complete_kimi_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: LoginCredentials::Kimi(tokens),
                })
                .map_err(flow_error)
        })),
    })
}

async fn start_openrouter(
    mode: InteractiveLoginMode,
) -> Result<InteractiveLogin, AuthenticationError> {
    if mode == InteractiveLoginMode::Device {
        return Err(AuthenticationError::Flow(
            "OpenRouter does not support device login; use browser login or an API key".into(),
        ));
    }
    Ok(InteractiveLogin {
        provider_label: "OpenRouter",
        user_action: InteractiveUserAction::BrowserOpened,
        completion: InteractiveLoginCompletion::Confirm(Box::pin(async {
            openrouter_oauth::run_openrouter_oauth_flow()
                .await
                .map(|key| CompletedAuthentication {
                    credentials: LoginCredentials::OpenRouter(key),
                })
                .map_err(flow_error)
        })),
    })
}

async fn start_xai(mode: InteractiveLoginMode) -> Result<InteractiveLogin, AuthenticationError> {
    if mode == InteractiveLoginMode::Browser {
        return Ok(InteractiveLogin {
            provider_label: "xAI",
            user_action: InteractiveUserAction::BrowserOpened,
            completion: InteractiveLoginCompletion::Confirm(Box::pin(async {
                xai_oauth::run_xai_oauth_flow()
                    .await
                    .map(|tokens| CompletedAuthentication {
                        credentials: LoginCredentials::Xai(tokens),
                    })
                    .map_err(flow_error)
            })),
        });
    }

    let login = xai_oauth::start_xai_device_login()
        .await
        .map_err(flow_error)?;
    let user_action = InteractiveUserAction::DeviceCode {
        verification_uri: login.verification_uri.clone(),
        user_code: login.user_code.clone(),
        verification_uri_complete: login.verification_uri_complete.clone(),
    };
    Ok(InteractiveLogin {
        provider_label: "xAI",
        user_action,
        completion: InteractiveLoginCompletion::Confirm(Box::pin(async move {
            xai_oauth::complete_xai_device_login(login)
                .await
                .map(|tokens| CompletedAuthentication {
                    credentials: LoginCredentials::Xai(tokens),
                })
                .map_err(flow_error)
        })),
    })
}

async fn start_ollama_device(
    mode: InteractiveLoginMode,
) -> Result<InteractiveLogin, AuthenticationError> {
    let open_browser = mode == InteractiveLoginMode::Browser;
    let login = ollama_device::start_ollama_device_login(/* open_browser */ open_browser)
        .await
        .map_err(flow_error)?;
    Ok(ollama_interactive_login(login, open_browser))
}

fn ollama_interactive_login(
    login: ollama_device::OllamaDeviceLogin,
    open_browser: bool,
) -> InteractiveLogin {
    let user_action = if open_browser {
        InteractiveUserAction::BrowserOpened
    } else {
        InteractiveUserAction::OpenUrl {
            url: login.connect_url,
            instruction: "Open this URL and approve the device for Ollama Cloud.".into(),
        }
    };
    InteractiveLogin {
        provider_label: "Ollama Cloud",
        user_action,
        completion: InteractiveLoginCompletion::Unconfirmed {
            instruction: "Approve the device in your browser, then use an Ollama Cloud model. Rho does not receive a completion callback.",
        },
    }
}

fn flow_error(error: impl std::fmt::Display) -> AuthenticationError {
    AuthenticationError::Flow(error.to_string())
}

#[cfg(test)]
#[path = "login_dispatch_tests.rs"]
mod tests;
