use std::{future::Future, pin::Pin, time::SystemTime};

use serde::Deserialize;
use thiserror::Error;

use {
    rho_providers::auth::{kimi_oauth::refresh_kimi_tokens, xai_token::refresh_xai_tokens},
    rho_providers::credentials::{
        load_codex_tokens, load_kimi_tokens, load_xai_tokens, save_kimi_tokens, save_xai_tokens,
        CodexTokens, CredentialStore, KimiTokens, XaiTokens,
    },
    rho_providers::providers::openai::auth::{refresh_codex_token, CodexAuthSource},
};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_ACCOUNT_HEADER: &str = "ChatGPT-Account-Id";
const KIMI_USAGE_URL: &str = "https://api.kimi.com/coding/v1/usages";
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

/// Boxed future returned by [`UsageLimitsSource::fetch`].
type UsageLimitsFuture<'a> = Pin<
    Box<dyn Future<Output = Result<Option<ProviderUsageLimits>, UsageLimitsError>> + Send + 'a>,
>;

#[derive(Debug, Error)]
pub enum UsageLimitsError {
    #[error("could not load credentials: {0}")]
    Credentials(#[from] rho_providers::credentials::CredentialError),
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

impl UsageLimitsError {
    /// Labels a transport failure with the provider that produced it.
    fn request(provider: &'static str) -> impl Fn(reqwest::Error) -> Self {
        move |source| Self::Request { provider, source }
    }
}

/// Rejects error statuses and decodes a provider usage payload.
async fn decode_usage_payload<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    provider: &'static str,
) -> Result<T, UsageLimitsError> {
    response
        .error_for_status()
        .map_err(UsageLimitsError::request(provider))?
        .json::<T>()
        .await
        .map_err(UsageLimitsError::request(provider))
}

/// Finishes the request/refresh/retry ladder shared by every usage source.
///
/// `first` is the initial response. When it fails auth and `refresh_and_retry`
/// returns a new response, that response is used instead. A still-failing
/// status becomes [`UsageLimitsError::Unauthorized`].
async fn finish_auth_retry<RefreshFut>(
    provider: &'static str,
    login: &'static str,
    is_auth_failure: impl Fn(reqwest::StatusCode) -> bool,
    first: Result<reqwest::Response, reqwest::Error>,
    refresh_and_retry: impl FnOnce() -> RefreshFut,
) -> Result<reqwest::Response, UsageLimitsError>
where
    RefreshFut: Future<Output = Result<Option<reqwest::Response>, UsageLimitsError>>,
{
    let mut response = first.map_err(UsageLimitsError::request(provider))?;
    if is_auth_failure(response.status()) {
        if let Some(retry) = refresh_and_retry().await? {
            response = retry;
        }
    }
    if is_auth_failure(response.status()) {
        return Err(UsageLimitsError::Unauthorized { provider, login });
    }
    Ok(response)
}

fn unauthorized_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED
}

fn unauthorized_or_forbidden(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN
}

/// HTTP client plus the endpoint a usage source queries.
struct UsageEndpoint {
    client: reqwest::Client,
    url: String,
}

impl UsageEndpoint {
    fn new(client: reqwest::Client, url: &str) -> Self {
        Self {
            client,
            url: url.into(),
        }
    }

    #[cfg(test)]
    fn with_url(url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
        }
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn get(&self) -> reqwest::RequestBuilder {
        self.client.get(&self.url)
    }
}

/// Supplies normalized OAuth usage windows for one connected provider.
///
/// Implementors should return only limits reported by the provider. Missing
/// windows must not be synthesized because an absent window may be temporary.
pub trait UsageLimitsSource {
    fn fetch<'a>(&'a self, store: &'a dyn CredentialStore) -> UsageLimitsFuture<'a>;
}

pub struct CodexUsageLimitsSource {
    endpoint: UsageEndpoint,
}

impl CodexUsageLimitsSource {
    const PROVIDER: &'static str = "Codex";
    const LOGIN: &'static str = "/login openai-codex";

    pub fn new(client: reqwest::Client) -> Self {
        Self {
            endpoint: UsageEndpoint::new(client, CODEX_USAGE_URL),
        }
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: String) -> Self {
        Self {
            endpoint: UsageEndpoint::with_url(endpoint),
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
            .endpoint
            .get()
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
        let response = finish_auth_retry(
            Self::PROVIDER,
            Self::LOGIN,
            unauthorized_status,
            self.request(&tokens).await,
            || async {
                let Some(refresh_token) = tokens.refresh_token.clone() else {
                    return Ok(None);
                };
                tokens = refresh_codex_token(
                    self.endpoint.client(),
                    store,
                    &refresh_token,
                    source,
                    &tokens,
                )
                .await
                .map_err(|err| UsageLimitsError::Refresh {
                    provider: Self::PROVIDER,
                    detail: err.to_string(),
                })?;
                self.request(&tokens)
                    .await
                    .map(Some)
                    .map_err(UsageLimitsError::request(Self::PROVIDER))
            },
        )
        .await?;
        let payload: CodexUsagePayload = decode_usage_payload(response, Self::PROVIDER).await?;
        Ok(ProviderUsageLimits {
            provider: Self::PROVIDER.into(),
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
    fn fetch<'a>(&'a self, store: &'a dyn CredentialStore) -> UsageLimitsFuture<'a> {
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

pub struct KimiUsageLimitsSource {
    endpoint: UsageEndpoint,
}

impl KimiUsageLimitsSource {
    const PROVIDER: &'static str = "Kimi Code";
    const LOGIN: &'static str = "/login kimi-code";

    pub fn new(client: reqwest::Client) -> Self {
        Self {
            endpoint: UsageEndpoint::new(client, KIMI_USAGE_URL),
        }
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: String) -> Self {
        Self {
            endpoint: UsageEndpoint::with_url(endpoint),
        }
    }

    fn configured_tokens(
        store: &dyn CredentialStore,
    ) -> Result<Option<(KimiTokens, KimiAuthSource)>, UsageLimitsError> {
        Self::configured_tokens_from(store, std::env::var("KIMI_ACCESS_TOKEN").ok())
    }

    fn configured_tokens_from(
        store: &dyn CredentialStore,
        env_access_token: Option<String>,
    ) -> Result<Option<(KimiTokens, KimiAuthSource)>, UsageLimitsError> {
        if let Some(access_token) = env_access_token.filter(|token| !token.trim().is_empty()) {
            return Ok(Some((
                KimiTokens {
                    access_token,
                    refresh_token: None,
                    expires_at_unix: None,
                    scope: String::new(),
                    token_type: "Bearer".into(),
                    expires_in: None,
                },
                KimiAuthSource::Env,
            )));
        }
        Ok(load_kimi_tokens(store)?.map(|tokens| (tokens, KimiAuthSource::Store)))
    }

    async fn request(&self, tokens: &KimiTokens) -> Result<reqwest::Response, reqwest::Error> {
        self.endpoint
            .get()
            .bearer_auth(&tokens.access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
    }

    async fn fetch_with_tokens(
        &self,
        store: &dyn CredentialStore,
        mut tokens: KimiTokens,
        source: KimiAuthSource,
    ) -> Result<ProviderUsageLimits, UsageLimitsError> {
        let response = finish_auth_retry(
            Self::PROVIDER,
            Self::LOGIN,
            unauthorized_status,
            self.request(&tokens).await,
            || async {
                if source != KimiAuthSource::Store {
                    return Ok(None);
                }
                let Some(refresh_token) = tokens.refresh_token.clone() else {
                    return Ok(None);
                };
                tokens = refresh_kimi_tokens(self.endpoint.client(), &refresh_token)
                    .await
                    .map_err(|error| UsageLimitsError::Refresh {
                        provider: Self::PROVIDER,
                        detail: error.to_string(),
                    })?;
                save_kimi_tokens(store, &tokens)?;
                self.request(&tokens)
                    .await
                    .map(Some)
                    .map_err(UsageLimitsError::request(Self::PROVIDER))
            },
        )
        .await?;
        let payload: KimiUsagePayload = decode_usage_payload(response, Self::PROVIDER).await?;
        Ok(ProviderUsageLimits {
            provider: Self::PROVIDER.into(),
            windows: payload.windows(),
        })
    }
}

impl UsageLimitsSource for KimiUsageLimitsSource {
    fn fetch<'a>(&'a self, store: &'a dyn CredentialStore) -> UsageLimitsFuture<'a> {
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
    endpoint: UsageEndpoint,
}

impl XaiUsageLimitsSource {
    const PROVIDER: &'static str = "xAI";
    const LOGIN: &'static str = "/login xai-oauth";

    pub fn new(client: reqwest::Client) -> Self {
        Self {
            endpoint: UsageEndpoint::new(client, XAI_BILLING_URL),
        }
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: String) -> Self {
        Self {
            endpoint: UsageEndpoint::with_url(endpoint),
        }
    }

    fn configured_tokens(
        store: &dyn CredentialStore,
    ) -> Result<Option<(XaiTokens, XaiAuthSource)>, UsageLimitsError> {
        Self::configured_tokens_from(store, std::env::var("XAI_ACCESS_TOKEN").ok())
    }

    fn configured_tokens_from(
        store: &dyn CredentialStore,
        env_access_token: Option<String>,
    ) -> Result<Option<(XaiTokens, XaiAuthSource)>, UsageLimitsError> {
        if let Some(access_token) = env_access_token.filter(|token| !token.trim().is_empty()) {
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
        self.endpoint
            .get()
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
        let response = finish_auth_retry(
            Self::PROVIDER,
            Self::LOGIN,
            unauthorized_or_forbidden,
            self.request(&tokens).await,
            || async {
                if source != XaiAuthSource::Store {
                    return Ok(None);
                }
                let Some(refresh_token) = tokens.refresh_token.clone() else {
                    return Ok(None);
                };
                tokens = refresh_xai_token(self.endpoint.client(), store, &refresh_token, &tokens)
                    .await
                    .map_err(|detail| UsageLimitsError::Refresh {
                        provider: Self::PROVIDER,
                        detail,
                    })?;
                self.request(&tokens)
                    .await
                    .map(Some)
                    .map_err(UsageLimitsError::request(Self::PROVIDER))
            },
        )
        .await?;
        let payload: XaiBillingPayload = decode_usage_payload(response, Self::PROVIDER).await?;
        Ok(ProviderUsageLimits {
            provider: Self::PROVIDER.into(),
            windows: payload.windows(),
        })
    }
}

impl UsageLimitsSource for XaiUsageLimitsSource {
    fn fetch<'a>(&'a self, store: &'a dyn CredentialStore) -> UsageLimitsFuture<'a> {
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
    client: reqwest::Client,
) -> Result<(ProviderLimits, Vec<UsageLimitsError>), UsageLimitsError> {
    let codex = CodexUsageLimitsSource::new(client.clone());
    let kimi = KimiUsageLimitsSource::new(client.clone());
    let xai = XaiUsageLimitsSource::new(client);
    let (codex, kimi, xai) = tokio::join!(codex.fetch(store), kimi.fetch(store), xai.fetch(store));
    aggregate_usage_limits([codex, kimi, xai])
}

#[cfg(test)]
async fn fetch_usage_limits_from_sources(
    store: &dyn CredentialStore,
    first: &(dyn UsageLimitsSource + Sync),
    second: &(dyn UsageLimitsSource + Sync),
) -> Result<(ProviderLimits, Vec<UsageLimitsError>), UsageLimitsError> {
    let (first, second) = tokio::join!(first.fetch(store), second.fetch(store));
    aggregate_usage_limits([first, second])
}

fn aggregate_usage_limits(
    results: impl IntoIterator<Item = Result<Option<ProviderUsageLimits>, UsageLimitsError>>,
) -> Result<(ProviderLimits, Vec<UsageLimitsError>), UsageLimitsError> {
    let mut providers = Vec::new();
    let mut errors = Vec::new();
    let mut saw_connected = false;
    for result in results {
        match result {
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
    providers.sort_by(|left, right| {
        left.provider
            .to_ascii_lowercase()
            .cmp(&right.provider.to_ascii_lowercase())
    });
    if !saw_connected {
        return Ok((ProviderLimits { providers }, errors));
    }
    if providers.is_empty() {
        return Err(errors.into_iter().next().expect("connected provider error"));
    }
    Ok((ProviderLimits { providers }, errors))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XaiAuthSource {
    Env,
    Store,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KimiAuthSource {
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
struct KimiUsagePayload {
    usage: Option<KimiUsageDetail>,
    #[serde(default)]
    limits: Vec<KimiUsageLimit>,
}

#[derive(Deserialize)]
struct KimiUsageLimit {
    window: Option<KimiUsageWindow>,
    detail: KimiUsageDetail,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiUsageWindow {
    duration: i64,
    time_unit: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiUsageDetail {
    limit: KimiNumber,
    used: Option<KimiNumber>,
    remaining: Option<KimiNumber>,
    reset_time: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KimiNumber {
    Number(f64),
    String(String),
}

impl KimiNumber {
    fn value(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::String(value) => value.parse().ok(),
        }
    }
}

impl KimiUsagePayload {
    fn windows(self) -> Vec<UsageLimitWindow> {
        self.limits
            .into_iter()
            .filter_map(|limit| {
                let label = limit
                    .window
                    .as_ref()
                    .and_then(KimiUsageWindow::label)
                    .unwrap_or_else(|| "Usage".into());
                limit.detail.into_window(label)
            })
            .chain(
                self.usage
                    .into_iter()
                    .filter_map(|detail| detail.into_window("Weekly".into())),
            )
            .collect()
    }
}

impl KimiUsageWindow {
    fn label(&self) -> Option<String> {
        let seconds = if self.time_unit.contains("MINUTE") {
            self.duration.checked_mul(60)?
        } else if self.time_unit.contains("HOUR") {
            self.duration.checked_mul(60 * 60)?
        } else if self.time_unit.contains("DAY") {
            self.duration.checked_mul(24 * 60 * 60)?
        } else {
            return None;
        };
        Some(window_label(seconds))
    }
}

impl KimiUsageDetail {
    fn into_window(self, label: String) -> Option<UsageLimitWindow> {
        let limit = self.limit.value()?;
        if limit <= 0.0 {
            return None;
        }
        let remaining = self
            .remaining
            .as_ref()
            .and_then(KimiNumber::value)
            .or_else(|| {
                self.used
                    .as_ref()
                    .and_then(KimiNumber::value)
                    .map(|used| limit - used)
            })?;
        Some(UsageLimitWindow {
            label,
            remaining_percent: (remaining / limit * 100.0).clamp(0.0, 100.0),
            resets_at_unix: parse_unix_timestamp(&self.reset_time)?,
        })
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
    #[serde(rename = "type")]
    kind: Option<String>,
    end: Option<String>,
}

impl XaiBillingPayload {
    fn windows(self) -> Vec<UsageLimitWindow> {
        let config = self.config.unwrap_or(self.root);
        let Some(used_percent) = config.credit_usage_percent else {
            return Vec::new();
        };
        let period = config.current_period;
        let label = period
            .as_ref()
            .and_then(|period| period.kind.as_deref())
            .map(xai_period_label)
            .unwrap_or("Usage");
        let Some(resets_at_unix) = period
            .and_then(|period| period.end)
            .or(config.billing_period_end)
            .and_then(|value| parse_unix_timestamp(&value))
        else {
            return Vec::new();
        };
        vec![UsageLimitWindow {
            label: label.into(),
            remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
            resets_at_unix,
        }]
    }
}

fn xai_period_label(kind: &str) -> &'static str {
    match kind {
        "USAGE_PERIOD_TYPE_WEEKLY" => "Weekly",
        "USAGE_PERIOD_TYPE_MONTHLY" => "Monthly",
        "USAGE_PERIOD_TYPE_DAILY" => "Daily",
        _ => "Usage",
    }
}

async fn refresh_xai_token(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    refresh_token: &str,
    previous: &XaiTokens,
) -> Result<XaiTokens, String> {
    let refreshed = refresh_xai_tokens(client, refresh_token, previous)
        .await
        .map_err(|err| err.to_string())?;
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
