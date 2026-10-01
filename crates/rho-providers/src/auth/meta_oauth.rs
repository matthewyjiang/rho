//! Muse subscription device login and Model API key minting.
//!
//! The device flow returns an identity token. That token cannot call the model.
//! `POST /muse-code/key` exchanges it for a short-lived Model API key. Meta does
//! not return a key lifetime; minted keys are treated as valid for 24 hours,
//! and a 401 remints earlier. The identity token itself is not refreshable.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{header::ACCEPT, StatusCode};
use serde::Deserialize;

use crate::{credentials::MetaTokens, model::TransportError};

use super::device_code::{
    poll_device_token, standard_device_poll_step, DeviceCodeResponse, DevicePollHttp,
    DevicePollOutcome, DevicePollRequest, DevicePollStep, FirstPoll,
};

pub(crate) const CLIENT_ID: &str = "1031625952748946";
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.meta.com/oidc/device/authorization/";
const DEVICE_TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";
pub(crate) const API_KEY_MINT_URL: &str = "https://api.meta.ai/muse-code/key";
/// Shown when a stored identity token can no longer mint a Model API key.
pub(crate) const SESSION_EXPIRED: &str =
    "Muse subscription session expired; run /login meta-muse again";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const API_VERSION_HEADER: &str = "x-api-version";
const API_VERSION: &str = "1.0.0";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Observed lifetime of a minted Muse key. The mint response does not include one.
pub(crate) const API_KEY_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_EXPIRES_IN: u64 = 15 * 60;
const DEFAULT_INTERVAL: u64 = 5;

#[derive(Clone)]
pub struct MetaDeviceLogin {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    device_code: String,
    expires_in: Duration,
    interval: Duration,
}

impl std::fmt::Debug for MetaDeviceLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetaDeviceLogin")
            .field("user_code", &"[REDACTED]")
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &"[REDACTED]")
            .field("device_code", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MetaOAuthError {
    #[error("Meta login request failed: {0}")]
    Request(#[source] TransportError),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Setup(String),
    #[error("Meta device login failed: {0}")]
    Device(String),
    #[error("timed out waiting for Meta device login")]
    Timeout,
    #[error("Meta login response was missing {0}")]
    MissingToken(&'static str),
}

impl From<reqwest::Error> for MetaOAuthError {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(TransportError::from_reqwest(error))
    }
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct MintResponse {
    api_key: Option<String>,
    action_url: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    detail: Option<String>,
    message: Option<String>,
}

pub async fn start_meta_device_login() -> Result<MetaDeviceLogin, MetaOAuthError> {
    start_with_endpoint(&client()?, DEVICE_AUTHORIZATION_URL).await
}

pub async fn complete_meta_device_login(
    login: MetaDeviceLogin,
) -> Result<MetaTokens, MetaOAuthError> {
    let http = client()?;
    let identity = complete_with_endpoint(&http, &login, DEVICE_TOKEN_URL).await?;
    mint_meta_tokens(&http, &identity, API_KEY_MINT_URL).await
}

/// Mint a Model API key from a stored identity token and stamp a 24 hour expiry.
pub(crate) async fn mint_meta_tokens(
    client: &reqwest::Client,
    identity_token: &str,
    mint_url: &str,
) -> Result<MetaTokens, MetaOAuthError> {
    let api_key = mint_api_key(client, identity_token, mint_url).await?;
    Ok(MetaTokens {
        identity_token: identity_token.to_string(),
        api_key,
        api_key_expires_at_unix: now_unix() + API_KEY_LIFETIME.as_secs() as i64,
    })
}

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn client() -> Result<reqwest::Client, MetaOAuthError> {
    Ok(crate::reqwest_client_builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(crate::rho_user_agent())
        .build()?)
}

async fn start_with_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
) -> Result<MetaDeviceLogin, MetaOAuthError> {
    let response = client
        .post(endpoint)
        .header(ACCEPT, "application/json")
        .form(&[("client_id", CLIENT_ID)])
        .send()
        .await?;
    let status = response.status();
    let body: DeviceCodeResponse = response.json().await?;
    if !status.is_success() {
        return Err(MetaOAuthError::Device(oauth_error(
            body.error,
            body.error_description,
        )));
    }
    let verification_uri_complete = body.verification_uri_complete.filter(|uri| !uri.is_empty());
    let verification_uri = body
        .verification_uri
        .filter(|uri| !uri.is_empty())
        .or_else(|| verification_uri_complete.clone())
        .ok_or(MetaOAuthError::MissingToken("verification_uri"))?;
    Ok(MetaDeviceLogin {
        device_code: required(body.device_code, "device_code")?,
        user_code: required(body.user_code, "user_code")?,
        verification_uri,
        verification_uri_complete,
        expires_in: Duration::from_secs(body.expires_in.unwrap_or(DEFAULT_EXPIRES_IN)),
        interval: Duration::from_secs(body.interval.unwrap_or(DEFAULT_INTERVAL).max(1)),
    })
}

async fn complete_with_endpoint(
    client: &reqwest::Client,
    login: &MetaDeviceLogin,
    endpoint: &str,
) -> Result<String, MetaOAuthError> {
    let form = [
        ("client_id", CLIENT_ID),
        ("device_code", login.device_code.as_str()),
        ("grant_type", DEVICE_GRANT),
    ];
    let extra_headers = [("accept", "application/json")];
    match poll_device_token(
        DevicePollRequest {
            client,
            endpoint,
            form: &form,
            extra_headers: &extra_headers,
            expires_in: login.expires_in,
            interval: login.interval,
            first_poll: FirstPoll::AfterInterval,
            http: DevicePollHttp::Json,
        },
        interpret_device_token,
    )
    .await?
    {
        DevicePollOutcome::Tokens(body) => required(body.access_token, "access_token"),
        DevicePollOutcome::Denied { description } => Err(MetaOAuthError::Device(description)),
        DevicePollOutcome::Timeout => Err(MetaOAuthError::Timeout),
        DevicePollOutcome::Fatal { error } => Err(MetaOAuthError::Device(error)),
    }
}

fn interpret_device_token(
    status: StatusCode,
    body: DeviceTokenResponse,
) -> DevicePollStep<DeviceTokenResponse> {
    if status.is_success()
        && body
            .access_token
            .as_deref()
            .is_some_and(|token| !token.is_empty())
    {
        return DevicePollStep::Tokens(body);
    }
    if let Some(step) = standard_device_poll_step(body.error.as_deref()) {
        return step;
    }
    if body.error.as_deref() == Some("access_denied") {
        return DevicePollStep::Denied {
            description: "Meta login was denied".into(),
        };
    }
    DevicePollStep::Fatal {
        error: oauth_error(body.error, body.error_description),
    }
}

async fn mint_api_key(
    client: &reqwest::Client,
    identity_token: &str,
    mint_url: &str,
) -> Result<String, MetaOAuthError> {
    let response = client
        .post(mint_url)
        .header(ACCEPT, "application/json")
        .bearer_auth(identity_token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(API_VERSION_HEADER, API_VERSION)
        .body("{}")
        .send()
        .await?;
    let status = response.status();
    let body: MintResponse = response.json().await?;
    if !status.is_success() {
        let detail = mint_detail(&body);
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(MetaOAuthError::Unauthorized(session_expired(detail)));
        }
        return Err(MetaOAuthError::Device(format!(
            "Meta API-key mint failed (HTTP {status}){detail}"
        )));
    }
    let api_key = body.api_key.as_deref().filter(|key| !key.is_empty());
    match api_key {
        Some(api_key) => Ok(api_key.to_string()),
        None => Err(MetaOAuthError::Setup(missing_key_message(&body))),
    }
}

fn session_expired(detail: String) -> String {
    if detail.is_empty() {
        SESSION_EXPIRED.to_string()
    } else {
        format!("{SESSION_EXPIRED}{detail}")
    }
}

fn missing_key_message(body: &MintResponse) -> String {
    let mut message = "Meta did not issue a Muse API key".to_string();
    if let Some(url) = body.action_url.as_deref().filter(|url| !url.is_empty()) {
        message.push_str(". Complete setup at ");
        message.push_str(url);
        message.push('.');
    } else {
        message.push_str(&mint_detail(body));
    }
    message
}

fn mint_detail(body: &MintResponse) -> String {
    let detail = body
        .error_description
        .as_deref()
        .or(body.detail.as_deref())
        .or(body.message.as_deref())
        .or(body.error.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match detail {
        Some(detail) => format!(": {detail}"),
        None => String::new(),
    }
}

fn required(value: Option<String>, field: &'static str) -> Result<String, MetaOAuthError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(MetaOAuthError::MissingToken(field))
}

fn oauth_error(error: Option<String>, description: Option<String>) -> String {
    description
        .filter(|value| !value.is_empty())
        .or(error)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "request failed".into())
}

#[cfg(test)]
#[path = "meta_oauth_tests.rs"]
mod tests;
