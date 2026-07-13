use std::{future::Future, pin::Pin, time::SystemTime};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    credentials::{load_codex_tokens, CodexTokens, CredentialStore},
    model::openai::auth::{refresh_codex_token, CodexAuthSource},
};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_ACCOUNT_HEADER: &str = "ChatGPT-Account-Id";

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderLimits {
    pub provider: String,
    pub windows: Vec<UsageLimitWindow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageLimitWindow {
    pub label: String,
    pub remaining_percent: f64,
    pub resets_at_unix: i64,
}

#[derive(Debug, Error)]
pub enum UsageLimitsError {
    #[error("could not load credentials: {0}")]
    Credentials(#[from] crate::credentials::CredentialError),
    #[error("Codex usage request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("could not refresh Codex OAuth credentials: {0}")]
    Refresh(String),
    #[error("Codex OAuth credentials are no longer valid; run /login openai-codex")]
    Unauthorized,
}

/// Supplies normalized OAuth usage windows for one connected provider.
///
/// Implementors should return only limits reported by the provider. Missing
/// windows must not be synthesized because an absent window may be temporary.
pub trait UsageLimitsSource {
    fn fetch<'a>(
        &'a self,
        store: &'a dyn CredentialStore,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ProviderLimits>, UsageLimitsError>> + Send + 'a>>;
}

pub struct CodexUsageLimitsSource {
    client: reqwest::Client,
    endpoint: String,
}

impl CodexUsageLimitsSource {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: CODEX_USAGE_URL.into(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
        }
    }

    fn configured_tokens(
        store: &dyn CredentialStore,
    ) -> Result<Option<(CodexTokens, CodexAuthSource)>, UsageLimitsError> {
        if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
            return Ok(Some((
                CodexTokens {
                    access_token,
                    refresh_token: None,
                    id_token: None,
                    account_id: std::env::var("CODEX_ACCOUNT_ID").ok(),
                },
                CodexAuthSource::Env,
            )));
        }
        Ok(load_codex_tokens(store)?.map(|tokens| (tokens, CodexAuthSource::Store)))
    }

    async fn request(&self, tokens: &CodexTokens) -> Result<reqwest::Response, reqwest::Error> {
        let mut request = self
            .client
            .get(&self.endpoint)
            .bearer_auth(&tokens.access_token)
            .header(reqwest::header::CACHE_CONTROL, "no-store");
        if let Some(account_id) = &tokens.account_id {
            request = request.header(CODEX_ACCOUNT_HEADER, account_id);
        }
        request.send().await
    }

    async fn fetch_with_tokens(
        &self,
        store: &dyn CredentialStore,
        mut tokens: CodexTokens,
        source: CodexAuthSource,
    ) -> Result<ProviderLimits, UsageLimitsError> {
        let mut response = self.request(&tokens).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(refresh_token) = tokens.refresh_token.clone() {
                tokens = refresh_codex_token(&self.client, store, &refresh_token, source, &tokens)
                    .await
                    .map_err(|err| UsageLimitsError::Refresh(err.to_string()))?;
                response = self.request(&tokens).await?;
            }
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UsageLimitsError::Unauthorized);
        }
        let payload = response
            .error_for_status()?
            .json::<CodexUsagePayload>()
            .await?;
        Ok(ProviderLimits {
            provider: "Codex".into(),
            windows: payload
                .rate_limit
                .into_iter()
                .flat_map(|limits| [limits.primary_window, limits.secondary_window])
                .flatten()
                .map(UsageLimitWindow::from)
                .collect(),
        })
    }
}

impl UsageLimitsSource for CodexUsageLimitsSource {
    fn fetch<'a>(
        &'a self,
        store: &'a dyn CredentialStore,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ProviderLimits>, UsageLimitsError>> + Send + 'a>>
    {
        Box::pin(async move {
            let Some((tokens, source)) = Self::configured_tokens(store)? else {
                return Ok(None);
            };
            self.fetch_with_tokens(store, tokens, source)
                .await
                .map(Some)
        })
    }
}

#[derive(Deserialize)]
struct CodexUsagePayload {
    rate_limit: Option<CodexRateLimit>,
}

#[derive(Deserialize)]
struct CodexRateLimit {
    primary_window: Option<CodexLimitWindow>,
    secondary_window: Option<CodexLimitWindow>,
}

#[derive(Deserialize)]
struct CodexLimitWindow {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: i64,
}

impl From<CodexLimitWindow> for UsageLimitWindow {
    fn from(window: CodexLimitWindow) -> Self {
        Self {
            label: window_label(window.limit_window_seconds),
            remaining_percent: (100.0 - window.used_percent).clamp(0.0, 100.0),
            resets_at_unix: window.reset_at,
        }
    }
}

fn window_label(seconds: i64) -> String {
    const HOUR: i64 = 60 * 60;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    if approximately(seconds, 5 * HOUR) {
        "5-hour".into()
    } else if approximately(seconds, WEEK) {
        "Weekly".into()
    } else if approximately(seconds, DAY) {
        "Daily".into()
    } else if seconds >= DAY && seconds % DAY == 0 {
        format!("{}-day", seconds / DAY)
    } else if seconds >= HOUR && seconds % HOUR == 0 {
        format!("{}-hour", seconds / HOUR)
    } else {
        "Usage".into()
    }
}

fn approximately(actual: i64, expected: i64) -> bool {
    actual >= expected * 95 / 100 && actual <= expected * 105 / 100
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
#[path = "usage_limits_tests.rs"]
mod tests;
