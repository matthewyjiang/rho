use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{header::ACCEPT, StatusCode};
use serde::Deserialize;
use tokio::time::{sleep, Instant};

use crate::credentials::KimiTokens;

pub(crate) const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
pub(crate) const TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
const DEVICE_URL: &str = "https://auth.kimi.com/api/oauth/device_authorization";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REFRESH_ATTEMPTS: u32 = 3;

#[derive(Clone)]
pub struct KimiDeviceLogin {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    device_code: String,
    expires_in: Duration,
    interval: Duration,
}

impl std::fmt::Debug for KimiDeviceLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KimiDeviceLogin")
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
pub enum KimiOAuthError {
    #[error("Kimi OAuth request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Kimi OAuth credentials were rejected: {0}")]
    Unauthorized(String),
    #[error("Kimi device login failed: {0}")]
    Device(String),
    #[error("timed out waiting for Kimi device login")]
    Timeout,
    #[error("Kimi OAuth token response was missing or invalid: {0}")]
    InvalidToken(&'static str),
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    token_type: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn start_kimi_device_login() -> Result<KimiDeviceLogin, KimiOAuthError> {
    start_with_endpoint(&client()?, DEVICE_URL).await
}

pub async fn complete_kimi_device_login(
    login: KimiDeviceLogin,
) -> Result<KimiTokens, KimiOAuthError> {
    complete_with_endpoint(&client()?, login, TOKEN_URL).await
}

pub async fn refresh_kimi_tokens(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<KimiTokens, KimiOAuthError> {
    refresh_kimi_tokens_with_endpoint(client, refresh_token, TOKEN_URL).await
}

async fn refresh_kimi_tokens_with_endpoint(
    client: &reqwest::Client,
    refresh_token: &str,
    endpoint: &str,
) -> Result<KimiTokens, KimiOAuthError> {
    for attempt in 0..MAX_REFRESH_ATTEMPTS {
        let response = client
            .post(endpoint)
            .header(ACCEPT, "application/json")
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await;
        match response {
            Ok(response)
                if refresh_status_is_retryable(response.status())
                    && attempt + 1 < MAX_REFRESH_ATTEMPTS =>
            {
                sleep(refresh_backoff(attempt)).await;
            }
            Ok(response) => return parse_token(response).await,
            Err(_) if attempt + 1 < MAX_REFRESH_ATTEMPTS => {
                sleep(refresh_backoff(attempt)).await;
            }
            Err(error) => return Err(KimiOAuthError::Request(error)),
        }
    }
    unreachable!("refresh loop always returns on its final attempt")
}

fn refresh_status_is_retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn refresh_backoff(attempt: u32) -> Duration {
    Duration::from_secs(1_u64 << attempt)
}

fn client() -> Result<reqwest::Client, KimiOAuthError> {
    Ok(reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(crate::rho_user_agent())
        .build()?)
}

async fn start_with_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
) -> Result<KimiDeviceLogin, KimiOAuthError> {
    let response = client
        .post(endpoint)
        .header(ACCEPT, "application/json")
        .form(&[("client_id", CLIENT_ID)])
        .send()
        .await?;
    let status = response.status();
    let body: DeviceResponse = response.json().await?;
    if !status.is_success() {
        return Err(KimiOAuthError::Device(oauth_error(
            body.error,
            body.error_description,
        )));
    }
    let verification_uri_complete =
        required(body.verification_uri_complete, "verification_uri_complete")?;
    Ok(KimiDeviceLogin {
        device_code: required(body.device_code, "device_code")?,
        user_code: required(body.user_code, "user_code")?,
        verification_uri: body
            .verification_uri
            .filter(|uri| !uri.is_empty())
            .unwrap_or_else(|| verification_uri_complete.clone()),
        verification_uri_complete: Some(verification_uri_complete),
        expires_in: Duration::from_secs(body.expires_in.unwrap_or(15 * 60)),
        interval: Duration::from_secs(body.interval.unwrap_or(5).max(1)),
    })
}

async fn complete_with_endpoint(
    client: &reqwest::Client,
    login: KimiDeviceLogin,
    endpoint: &str,
) -> Result<KimiTokens, KimiOAuthError> {
    let deadline = Instant::now() + login.expires_in;
    let mut interval = login.interval;
    loop {
        if Instant::now() >= deadline {
            return Err(KimiOAuthError::Timeout);
        }
        let response = client
            .post(endpoint)
            .header(ACCEPT, "application/json")
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", login.device_code.as_str()),
                ("grant_type", DEVICE_GRANT),
            ])
            .send()
            .await?;
        if response.status().is_success() {
            return parse_token(response).await;
        }
        let body: TokenResponse = response.json().await?;
        match body.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => return Err(KimiOAuthError::Timeout),
            _ => {
                return Err(KimiOAuthError::Device(oauth_error(
                    body.error,
                    body.error_description,
                )))
            }
        }
        sleep(interval).await;
    }
}

async fn parse_token(response: reqwest::Response) -> Result<KimiTokens, KimiOAuthError> {
    let status = response.status();
    let body: TokenResponse = response.json().await?;
    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || body.error.as_deref() == Some("invalid_grant")
    {
        return Err(KimiOAuthError::Unauthorized(oauth_error(
            body.error,
            body.error_description,
        )));
    }
    if !status.is_success() {
        return Err(KimiOAuthError::Device(oauth_error(
            body.error,
            body.error_description,
        )));
    }
    let expires_in = body
        .expires_in
        .filter(|value| *value > 0)
        .ok_or(KimiOAuthError::InvalidToken("expires_in"))?;
    Ok(KimiTokens {
        access_token: required(body.access_token, "access_token")?,
        refresh_token: Some(required(body.refresh_token, "refresh_token")?),
        expires_at_unix: Some(now_unix() + expires_in as i64),
        scope: body.scope.unwrap_or_default(),
        token_type: body.token_type.unwrap_or_else(|| "Bearer".into()),
        expires_in: Some(expires_in),
    })
}

fn required(value: Option<String>, field: &'static str) -> Result<String, KimiOAuthError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(KimiOAuthError::InvalidToken(field))
}

fn oauth_error(error: Option<String>, description: Option<String>) -> String {
    description
        .or(error)
        .unwrap_or_else(|| "unknown error".into())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
#[path = "kimi_oauth_tests.rs"]
mod tests;
