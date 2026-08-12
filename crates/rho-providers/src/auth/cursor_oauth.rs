use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use rand::RngCore;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use url::Url;

use crate::credentials::CursorTokens;

pub(crate) const LOGIN_URL: &str = "https://cursor.com/loginDeepControl";
pub(crate) const POLL_URL: &str = "https://api2.cursor.sh/auth/poll";
pub(crate) const REFRESH_URL: &str = "https://api2.cursor.sh/auth/exchange_user_api_key";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_MAX_ATTEMPTS: u32 = 150;
const POLL_BASE_DELAY: Duration = Duration::from_millis(1_000);
const POLL_MAX_DELAY: Duration = Duration::from_secs(10);
const EXPIRY_SKEW_SECONDS: i64 = 5 * 60;
const FALLBACK_LIFETIME_SECONDS: i64 = 60 * 60;

#[derive(Clone)]
pub struct CursorOAuthLogin {
    pub login_url: String,
    uuid: String,
    verifier: String,
}

impl std::fmt::Debug for CursorOAuthLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CursorOAuthLogin")
            .field("login_url", &self.login_url)
            .field("uuid", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CursorOAuthError {
    #[error("could not open a browser for Cursor OAuth")]
    Browser,
    #[error("Cursor OAuth request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Cursor OAuth credentials were rejected: {0}")]
    Unauthorized(String),
    #[error("Cursor login failed: {0}")]
    Flow(String),
    #[error("timed out waiting for Cursor login")]
    Timeout,
    #[error("Cursor OAuth token response was missing or invalid: {0}")]
    InvalidToken(&'static str),
}

#[derive(Deserialize)]
struct PollResponse {
    #[serde(alias = "accessToken")]
    access_token: Option<String>,
    #[serde(alias = "refreshToken")]
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct RefreshResponse {
    #[serde(alias = "accessToken")]
    access_token: Option<String>,
    #[serde(alias = "refreshToken")]
    refresh_token: Option<String>,
}

pub fn start_cursor_login() -> CursorOAuthLogin {
    let mut verifier_bytes = [0u8; 96];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let uuid = random_uuid();
    let mut login_url = Url::parse(LOGIN_URL).expect("Cursor login URL is valid");
    login_url
        .query_pairs_mut()
        .append_pair("challenge", &challenge)
        .append_pair("uuid", &uuid)
        .append_pair("mode", "login")
        .append_pair("redirectTarget", "cli");
    CursorOAuthLogin {
        login_url: login_url.to_string(),
        uuid,
        verifier,
    }
}

#[cfg(test)]
impl CursorOAuthLogin {
    pub(crate) fn for_test(uuid: impl Into<String>, verifier: impl Into<String>) -> Self {
        Self {
            login_url: LOGIN_URL.into(),
            uuid: uuid.into(),
            verifier: verifier.into(),
        }
    }
}

pub async fn complete_cursor_login(
    login: CursorOAuthLogin,
) -> Result<CursorTokens, CursorOAuthError> {
    complete_with_endpoint(&client()?, login, POLL_URL).await
}

pub async fn refresh_cursor_tokens(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<CursorTokens, CursorOAuthError> {
    refresh_cursor_tokens_with_endpoint(client, refresh_token, REFRESH_URL).await
}

pub(crate) async fn refresh_cursor_tokens_with_endpoint(
    client: &reqwest::Client,
    refresh_token: &str,
    endpoint: &str,
) -> Result<CursorTokens, CursorOAuthError> {
    let response = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {refresh_token}"))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let body = response.text().await.unwrap_or_default();
        return Err(CursorOAuthError::Unauthorized(body));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CursorOAuthError::Flow(format!("HTTP {status}: {body}")));
    }
    let body: RefreshResponse = response.json().await?;
    let access_token = required(body.access_token, "accessToken")?;
    let expires_at_unix = token_expiry_unix(&access_token);
    Ok(CursorTokens {
        refresh_token: Some(
            body.refresh_token
                .filter(|token| !token.is_empty())
                .unwrap_or_else(|| refresh_token.to_string()),
        ),
        access_token,
        expires_at_unix: Some(expires_at_unix),
    })
}

pub(crate) async fn complete_with_endpoint(
    client: &reqwest::Client,
    login: CursorOAuthLogin,
    endpoint: &str,
) -> Result<CursorTokens, CursorOAuthError> {
    let mut delay = POLL_BASE_DELAY;
    let mut consecutive_errors = 0u32;
    for _ in 0..POLL_MAX_ATTEMPTS {
        let mut poll_url =
            Url::parse(endpoint).map_err(|error| CursorOAuthError::Flow(error.to_string()))?;
        poll_url
            .query_pairs_mut()
            .append_pair("uuid", &login.uuid)
            .append_pair("verifier", &login.verifier);
        let response = match client.get(poll_url).send().await {
            Ok(response) => response,
            Err(_) => {
                consecutive_errors += 1;
                if consecutive_errors >= 3 {
                    return Err(CursorOAuthError::Flow(
                        "too many consecutive errors during Cursor auth polling".into(),
                    ));
                }
                sleep(delay).await;
                continue;
            }
        };
        consecutive_errors = 0;
        match response.status() {
            StatusCode::NOT_FOUND => {
                sleep(delay).await;
                delay = std::cmp::min(
                    Duration::from_secs_f64(delay.as_secs_f64() * 1.2),
                    POLL_MAX_DELAY,
                );
            }
            status if status.is_success() => {
                let body: PollResponse = response.json().await?;
                let access_token = required(body.access_token, "accessToken")?;
                let refresh_token = required(body.refresh_token, "refreshToken")?;
                let expires_at_unix = token_expiry_unix(&access_token);
                return Ok(CursorTokens {
                    access_token,
                    refresh_token: Some(refresh_token),
                    expires_at_unix: Some(expires_at_unix),
                });
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                return Err(CursorOAuthError::Flow(format!("HTTP {status}: {body}")));
            }
        }
    }
    Err(CursorOAuthError::Timeout)
}

fn client() -> Result<reqwest::Client, CursorOAuthError> {
    Ok(reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(crate::rho_user_agent())
        .build()?)
}

fn required(value: Option<String>, field: &'static str) -> Result<String, CursorOAuthError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(CursorOAuthError::InvalidToken(field))
}

pub(crate) fn token_expiry_unix(access_token: &str) -> i64 {
    jwt_exp_unix(access_token)
        .map(|exp| exp.saturating_sub(EXPIRY_SKEW_SECONDS))
        .unwrap_or_else(|| now_unix() + FALLBACK_LIFETIME_SECONDS)
}

fn jwt_exp_unix(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp")?.as_i64()
}

fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
#[path = "cursor_oauth_tests.rs"]
mod tests;
