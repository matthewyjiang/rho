use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{sleep, timeout, Instant},
};
use url::Url;

use crate::credentials::XaiTokens;

pub(crate) const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub(crate) const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const AUTHORIZE_URL: &str = "https://auth.x.ai/oauth2/authorize";
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.x.ai/oauth2/device/code";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PORT: u16 = 56121;
const CALLBACK_PATH: &str = "/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DEFAULT_DEVICE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DEFAULT_DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct XaiOAuthRequest {
    pub authorize_url: String,
    redirect_uri: String,
    state: String,
    verifier: String,
    challenge: String,
}

#[derive(Clone, Debug)]
pub struct XaiDeviceLogin {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    expires_in: Duration,
    interval: Duration,
    device_code: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackOutcome {
    Code(String),
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum XaiOAuthError {
    #[error("could not bind local xAI OAuth callback listener: {0}")]
    Bind(std::io::Error),
    #[error("could not open browser for xAI OAuth: {0}")]
    Browser(String),
    #[error("timed out waiting for xAI OAuth browser callback")]
    Timeout,
    #[error("could not read xAI OAuth callback: {0}")]
    CallbackIo(std::io::Error),
    #[error("xAI OAuth callback was invalid: {0}")]
    InvalidCallback(String),
    #[error("xAI OAuth was denied or failed: {0}")]
    OAuthDenied(String),
    #[error("xAI device login setup failed: {0}")]
    DeviceSetup(String),
    #[error("timed out waiting for xAI device login")]
    DeviceTimeout,
    #[error("xAI OAuth request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("xAI OAuth token response was missing {0}")]
    MissingToken(&'static str),
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
    scope: &'a str,
}

pub async fn run_xai_oauth_flow() -> Result<XaiTokens, XaiOAuthError> {
    let client = http_client()?;
    let listener = TcpListener::bind((CALLBACK_HOST, CALLBACK_PORT))
        .await
        .map_err(XaiOAuthError::Bind)?;
    let request = build_oauth_request();
    webbrowser::open(&request.authorize_url)
        .map_err(|err| XaiOAuthError::Browser(err.to_string()))?;

    let code = match timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(&listener, &request.state),
    )
    .await
    {
        Ok(Ok(CallbackOutcome::Code(code))) => code,
        Ok(Ok(CallbackOutcome::Error(error))) => return Err(XaiOAuthError::OAuthDenied(error)),
        Ok(Err(err)) => return Err(err),
        Err(_) => return Err(XaiOAuthError::Timeout),
    };

    exchange_code(&client, &code, &request).await
}

pub async fn start_xai_device_login() -> Result<XaiDeviceLogin, XaiOAuthError> {
    start_xai_device_login_with_endpoint(&http_client()?, DEVICE_AUTHORIZATION_URL).await
}

pub async fn complete_xai_device_login(login: XaiDeviceLogin) -> Result<XaiTokens, XaiOAuthError> {
    complete_xai_device_login_with_endpoint(&http_client()?, login, TOKEN_URL).await
}

pub fn build_oauth_request() -> XaiOAuthRequest {
    build_oauth_request_with_values(random_token(32), random_token(64), random_token(32))
}

fn build_oauth_request_with_values(
    state: String,
    verifier: String,
    nonce: String,
) -> XaiOAuthRequest {
    let redirect_uri = format!("http://{CALLBACK_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}");
    let challenge = pkce_challenge(&verifier);
    let mut url = Url::parse(AUTHORIZE_URL).expect("xAI authorize URL must be valid");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("plan", "generic")
        .append_pair("referrer", "rho");
    XaiOAuthRequest {
        authorize_url: url.to_string(),
        redirect_uri,
        state,
        verifier,
        challenge,
    }
}

fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn http_client() -> Result<reqwest::Client, XaiOAuthError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("rho/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(XaiOAuthError::Request)
}

async fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<CallbackOutcome, XaiOAuthError> {
    let (mut stream, _) = listener.accept().await.map_err(XaiOAuthError::CallbackIo)?;
    let mut buffer = vec![0_u8; 8192];
    let len = stream
        .read(&mut buffer)
        .await
        .map_err(XaiOAuthError::CallbackIo)?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let outcome =
        parse_callback_request_line(request.lines().next().unwrap_or_default(), expected_state);
    let body = match &outcome {
        Ok(CallbackOutcome::Code(_)) => "xAI login complete. You can return to Rho.",
        Ok(CallbackOutcome::Error(_)) | Err(_) => {
            "xAI login failed. You can return to Rho for details."
        }
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    outcome
}

pub fn parse_callback_request_line(
    request_line: &str,
    expected_state: &str,
) -> Result<CallbackOutcome, XaiOAuthError> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" || target.is_empty() {
        return Err(XaiOAuthError::InvalidCallback(
            "expected GET callback request".into(),
        ));
    }
    let url = Url::parse(&format!("http://{CALLBACK_HOST}{target}")).map_err(|err| {
        XaiOAuthError::InvalidCallback(format!("callback URL could not be parsed: {err}"))
    })?;
    if url.path() != CALLBACK_PATH {
        return Err(XaiOAuthError::InvalidCallback(format!(
            "callback path was not {CALLBACK_PATH}"
        )));
    }
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    let state = query
        .get("state")
        .ok_or_else(|| XaiOAuthError::InvalidCallback("callback was missing state".into()))?;
    if state != expected_state {
        return Err(XaiOAuthError::InvalidCallback(
            "callback state did not match".into(),
        ));
    }
    if let Some(error) = query.get("error") {
        return Ok(CallbackOutcome::Error(
            query
                .get("error_description")
                .cloned()
                .unwrap_or_else(|| error.clone()),
        ));
    }
    query
        .get("code")
        .filter(|code| !code.is_empty())
        .cloned()
        .map(CallbackOutcome::Code)
        .ok_or_else(|| XaiOAuthError::InvalidCallback("callback was missing code".into()))
}

async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    request: &XaiOAuthRequest,
) -> Result<XaiTokens, XaiOAuthError> {
    exchange_code_with_endpoint(client, code, request, TOKEN_URL).await
}

async fn exchange_code_with_endpoint(
    client: &reqwest::Client,
    code: &str,
    request: &XaiOAuthRequest,
    endpoint: &str,
) -> Result<XaiTokens, XaiOAuthError> {
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", request.redirect_uri.as_str()),
            ("client_id", CLIENT_ID),
            ("code_verifier", request.verifier.as_str()),
            // xAI currently requires the original challenge to be echoed at token exchange.
            ("code_challenge", request.challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<TokenResponse>()
        .await?;
    tokens_from_response(response)
}

async fn start_xai_device_login_with_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
) -> Result<XaiDeviceLogin, XaiOAuthError> {
    let response = client
        .post(endpoint)
        .form(&DeviceCodeRequest {
            client_id: CLIENT_ID,
            scope: SCOPE,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<DeviceCodeResponse>()
        .await?;
    if let Some(error) = response.error {
        return Err(XaiOAuthError::DeviceSetup(
            response.error_description.unwrap_or(error),
        ));
    }
    Ok(XaiDeviceLogin {
        user_code: response
            .user_code
            .ok_or(XaiOAuthError::MissingToken("user_code"))?,
        verification_uri: response
            .verification_uri
            .ok_or(XaiOAuthError::MissingToken("verification_uri"))?,
        verification_uri_complete: response.verification_uri_complete,
        expires_in: Duration::from_secs(
            response
                .expires_in
                .unwrap_or(DEFAULT_DEVICE_TIMEOUT.as_secs()),
        ),
        interval: Duration::from_secs(
            response
                .interval
                .unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL.as_secs())
                .max(1),
        ),
        device_code: response
            .device_code
            .ok_or(XaiOAuthError::MissingToken("device_code"))?,
    })
}

async fn complete_xai_device_login_with_endpoint(
    client: &reqwest::Client,
    login: XaiDeviceLogin,
    endpoint: &str,
) -> Result<XaiTokens, XaiOAuthError> {
    let deadline = Instant::now() + login.expires_in;
    let mut interval = login.interval;
    while Instant::now() < deadline {
        let response = client
            .post(endpoint)
            .form(&[
                ("grant_type", DEVICE_CODE_GRANT_TYPE),
                ("client_id", CLIENT_ID),
                ("device_code", login.device_code.as_str()),
            ])
            .send()
            .await?;
        let status = response.status();
        let body = response.json::<TokenResponse>().await?;
        if status.is_success() {
            return tokens_from_response(body);
        }
        match body.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += SLOW_DOWN_INCREMENT,
            Some("access_denied" | "authorization_denied") => {
                return Err(XaiOAuthError::OAuthDenied(
                    body.error_description
                        .unwrap_or_else(|| "access denied".into()),
                ));
            }
            Some("expired_token") => return Err(XaiOAuthError::DeviceTimeout),
            Some(error) => {
                return Err(XaiOAuthError::DeviceSetup(
                    body.error_description.unwrap_or_else(|| error.to_string()),
                ));
            }
            None => {
                return Err(XaiOAuthError::DeviceSetup(format!(
                    "token endpoint returned HTTP {status} without an OAuth error"
                )));
            }
        }
        sleep(interval).await;
    }
    Err(XaiOAuthError::DeviceTimeout)
}

fn tokens_from_response(response: TokenResponse) -> Result<XaiTokens, XaiOAuthError> {
    let access_token = response
        .access_token
        .ok_or(XaiOAuthError::MissingToken("access_token"))?;
    let expires_at_unix = response.expires_in.and_then(|expires_in| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|now| i64::try_from(now.as_secs().saturating_add(expires_in)).ok())
    });
    Ok(XaiTokens {
        access_token,
        refresh_token: response.refresh_token,
        expires_at_unix,
        id_token: response.id_token,
    })
}

#[cfg(test)]
#[path = "xai_oauth_tests.rs"]
mod tests;
