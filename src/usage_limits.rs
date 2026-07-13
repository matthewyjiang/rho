use std::{future::Future, pin::Pin, time::SystemTime};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    auth::xai_oauth::{CLIENT_ID, TOKEN_URL},
    credentials::{
        load_codex_tokens, load_xai_tokens, save_xai_tokens, CodexTokens, CredentialStore,
        XaiTokens,
    },
    model::openai::auth::{refresh_codex_token, CodexAuthSource},
};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_ACCOUNT_HEADER: &str = "ChatGPT-Account-Id";
const XAI_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const XAI_TOKEN_AUTH_HEADER: &str = "xai-grok-cli";
const XAI_CLIENT_VERSION: &str = "0.2.93";

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderLimits {
    pub providers: Vec<ProviderUsageLimits>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderUsageLimits {
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
    #[error("{provider} usage request failed: {source}")]
    Request {
        provider: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("could not refresh {provider} OAuth credentials: {detail}")]
    Refresh {
        provider: &'static str,
        detail: String,
    },
    #[error("{provider} OAuth credentials are no longer valid; run {login}")]
    Unauthorized {
        provider: &'static str,
        login: &'static str,
    },
}

/// Supplies normalized OAuth usage windows for one connected provider.
///
/// Implementors should return only limits reported by the provider. Missing
/// windows must not be synthesized because an absent window may be temporary.
pub trait UsageLimitsSource {
    fn fetch<'a>(
        &'a self,
        store: &'a dyn CredentialStore,
    ) -> Pin<
        Box<dyn Future<Output = Result<Option<ProviderUsageLimits>, UsageLimitsError>> + Send + 'a>,
    >;
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
    ) -> Result<ProviderUsageLimits, UsageLimitsError> {
        let mut response =
            self.request(&tokens)
                .await
                .map_err(|source| UsageLimitsError::Request {
                    provider: "Codex",
                    source,
                })?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(refresh_token) = tokens.refresh_token.clone() {
                tokens = refresh_codex_token(&self.client, store, &refresh_token, source, &tokens)
                    .await
                    .map_err(|err| UsageLimitsError::Refresh {
                        provider: "Codex",
                        detail: err.to_string(),
                    })?;
                response =
                    self.request(&tokens)
                        .await
                        .map_err(|source| UsageLimitsError::Request {
                            provider: "Codex",
                            source,
                        })?;
            }
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UsageLimitsError::Unauthorized {
                provider: "Codex",
                login: "/login openai-codex",
            });
        }
        let payload = response
            .error_for_status()
            .map_err(|source| UsageLimitsError::Request {
                provider: "Codex",
                source,
            })?
            .json::<CodexUsagePayload>()
            .await
            .map_err(|source| UsageLimitsError::Request {
                provider: "Codex",
                source,
            })?;
        Ok(ProviderUsageLimits {
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
    ) -> Pin<
        Box<dyn Future<Output = Result<Option<ProviderUsageLimits>, UsageLimitsError>> + Send + 'a>,
    > {
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

pub struct XaiUsageLimitsSource {
    client: reqwest::Client,
    endpoint: String,
}

impl XaiUsageLimitsSource {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: XAI_BILLING_URL.into(),
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
    ) -> Result<Option<(XaiTokens, XaiAuthSource)>, UsageLimitsError> {
        if let Ok(access_token) = std::env::var("XAI_ACCESS_TOKEN") {
            return Ok(Some((
                XaiTokens {
                    access_token,
                    refresh_token: None,
                    expires_at_unix: None,
                    id_token: None,
                },
                XaiAuthSource::Env,
            )));
        }
        Ok(load_xai_tokens(store)?.map(|tokens| (tokens, XaiAuthSource::Store)))
    }

    async fn request(&self, tokens: &XaiTokens) -> Result<reqwest::Response, reqwest::Error> {
        self.client
            .get(&self.endpoint)
            .bearer_auth(&tokens.access_token)
            .header("x-xai-token-auth", XAI_TOKEN_AUTH_HEADER)
            .header("x-grok-client-version", XAI_CLIENT_VERSION)
            .header(
                reqwest::header::USER_AGENT,
                format!(
                    "rho/{}/grok-shell/{XAI_CLIENT_VERSION}",
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
    }

    async fn fetch_with_tokens(
        &self,
        store: &dyn CredentialStore,
        mut tokens: XaiTokens,
        source: XaiAuthSource,
    ) -> Result<ProviderUsageLimits, UsageLimitsError> {
        let mut response =
            self.request(&tokens)
                .await
                .map_err(|source| UsageLimitsError::Request {
                    provider: "xAI",
                    source,
                })?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            if source == XaiAuthSource::Store {
                if let Some(refresh_token) = tokens.refresh_token.clone() {
                    tokens = refresh_xai_token(&self.client, store, &refresh_token, &tokens)
                        .await
                        .map_err(|detail| UsageLimitsError::Refresh {
                            provider: "xAI",
                            detail,
                        })?;
                    response = self.request(&tokens).await.map_err(|source| {
                        UsageLimitsError::Request {
                            provider: "xAI",
                            source,
                        }
                    })?;
                }
            }
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(UsageLimitsError::Unauthorized {
                provider: "xAI",
                login: "/login xai",
            });
        }
        let payload = response
            .error_for_status()
            .map_err(|source| UsageLimitsError::Request {
                provider: "xAI",
                source,
            })?
            .json::<XaiBillingPayload>()
            .await
            .map_err(|source| UsageLimitsError::Request {
                provider: "xAI",
                source,
            })?;
        Ok(ProviderUsageLimits {
            provider: "xAI".into(),
            windows: payload.windows(),
        })
    }
}

impl UsageLimitsSource for XaiUsageLimitsSource {
    fn fetch<'a>(
        &'a self,
        store: &'a dyn CredentialStore,
    ) -> Pin<
        Box<dyn Future<Output = Result<Option<ProviderUsageLimits>, UsageLimitsError>> + Send + 'a>,
    > {
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

pub async fn fetch_connected_usage_limits(
    store: &dyn CredentialStore,
) -> Result<(ProviderLimits, Vec<UsageLimitsError>), UsageLimitsError> {
    let mut providers = Vec::new();
    let mut errors = Vec::new();
    let mut saw_connected = false;
    for source in connected_sources() {
        match source.fetch(store).await {
            Ok(None) => {}
            Ok(Some(limits)) => {
                saw_connected = true;
                providers.push(limits);
            }
            Err(error) => {
                saw_connected = true;
                errors.push(error);
            }
        }
    }
    if !saw_connected {
        return Ok((ProviderLimits { providers }, errors));
    }
    if providers.is_empty() {
        return Err(errors.into_iter().next().expect("connected provider error"));
    }
    Ok((ProviderLimits { providers }, errors))
}

fn connected_sources() -> Vec<Box<dyn UsageLimitsSource + Send + Sync>> {
    vec![
        Box::new(CodexUsageLimitsSource::new()),
        Box::new(XaiUsageLimitsSource::new()),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XaiAuthSource {
    Env,
    Store,
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

#[derive(Deserialize)]
struct XaiBillingPayload {
    config: Option<XaiBillingConfig>,
    #[serde(flatten)]
    root: XaiBillingConfig,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct XaiBillingConfig {
    credit_usage_percent: Option<f64>,
    current_period: Option<XaiBillingPeriod>,
    billing_period_end: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct XaiBillingPeriod {
    end: Option<String>,
}

impl XaiBillingPayload {
    fn windows(self) -> Vec<UsageLimitWindow> {
        let config = self.config.unwrap_or(self.root);
        let Some(used_percent) = config.credit_usage_percent else {
            return Vec::new();
        };
        let Some(resets_at_unix) = config
            .current_period
            .and_then(|period| period.end)
            .or(config.billing_period_end)
            .and_then(|value| parse_unix_timestamp(&value))
        else {
            return Vec::new();
        };
        vec![UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
            resets_at_unix,
        }]
    }
}

#[derive(Deserialize)]
struct XaiRefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

async fn refresh_xai_token(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    refresh_token: &str,
    previous: &XaiTokens,
) -> Result<XaiTokens, String> {
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    let response = response
        .json::<XaiRefreshResponse>()
        .await
        .map_err(|err| err.to_string())?;
    let access_token = response
        .access_token
        .ok_or_else(|| "xAI refresh response missing access_token".to_string())?;
    let refreshed = XaiTokens {
        access_token,
        refresh_token: Some(
            response
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_string()),
        ),
        expires_at_unix: response
            .expires_in
            .and_then(|expires| {
                i64::try_from(expires)
                    .ok()
                    .map(|expires| now_unix().saturating_add(expires))
            })
            .or(previous.expires_at_unix),
        id_token: response.id_token.or_else(|| previous.id_token.clone()),
    };
    save_xai_tokens(store, &refreshed).map_err(|err| err.to_string())?;
    Ok(refreshed)
}

fn parse_unix_timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.fZ")
                .ok()
                .map(|value| value.and_utc().timestamp())
        })
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
