use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, time::timeout};
use url::Url;

use crate::model::TransportError;

use super::loopback::{
    accept_request, bind_loopback, pkce_challenge, random_token, write_response, LoopbackBindError,
    ResponseBodies, ResponseKind,
};

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const KEY_EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const CALLBACK_PATH_PREFIX: &str = "/callback/";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

struct OpenRouterOAuthRequest {
    authorize_url: String,
    verifier: String,
}

impl std::fmt::Debug for OpenRouterOAuthRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenRouterOAuthRequest")
            .field("authorize_url", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenRouterOAuthError {
    #[error("could not bind local OpenRouter OAuth callback listener: {0}")]
    Bind(std::io::Error),
    #[error("could not determine the local OpenRouter OAuth callback address: {0}")]
    LocalAddress(std::io::Error),
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-providers): remove OpenRouterOAuthError::Browser; browser launch lives in the login dispatch layer
    #[error("could not open a browser for OpenRouter OAuth")]
    Browser,
    #[error("timed out waiting for the OpenRouter OAuth browser callback")]
    Timeout,
    #[error("could not accept an OpenRouter OAuth callback: {0}")]
    Accept(std::io::Error),
    #[error("OpenRouter OAuth was denied or failed: {0}")]
    OAuthDenied(String),
    #[error("the OpenRouter OAuth callback was invalid")]
    InvalidCallback,
    #[error("the OpenRouter OAuth key request failed: {0}")]
    Request(#[source] TransportError),
    #[error("the OpenRouter OAuth key endpoint returned HTTP {0}")]
    ExchangeStatus(http::StatusCode),
    #[error("the OpenRouter OAuth key response was invalid: {0}")]
    InvalidResponse(#[source] TransportError),
    #[error("the OpenRouter OAuth key response did not include a key")]
    MissingKey,
}

impl From<reqwest::Error> for OpenRouterOAuthError {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(TransportError::from_reqwest(error))
    }
}

#[derive(Serialize)]
struct KeyExchangeRequest<'a> {
    code: &'a str,
    code_verifier: &'a str,
    code_challenge_method: &'static str,
}

#[derive(Deserialize)]
struct KeyExchangeResponse {
    key: Option<String>,
}

#[derive(PartialEq, Eq)]
enum CallbackParse {
    Code(String),
    Denied(String),
    Ignored,
    Invalid,
}

impl std::fmt::Debug for CallbackParse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code(_) => formatter.write_str("Code([REDACTED])"),
            Self::Denied(_) => formatter.write_str("Denied([REDACTED])"),
            Self::Ignored => formatter.write_str("Ignored"),
            Self::Invalid => formatter.write_str("Invalid"),
        }
    }
}

/// Bound loopback login. The authorize URL is ready before the callback wait.
pub struct OpenRouterBrowserLogin {
    pub authorize_url: String,
    listener: TcpListener,
    callback_path: String,
    verifier: String,
}

/// One-shot browser login used by 2.0 callers.
///
/// # Next major
///
/// NEXT_MAJOR(rho-providers): remove run_openrouter_oauth_flow; use start_openrouter_browser_login and complete_openrouter_browser_login
#[deprecated(
    since = "2.1.0",
    note = "use start_openrouter_browser_login and complete_openrouter_browser_login so the authorize URL can be shown before the browser opens"
)]
pub async fn run_openrouter_oauth_flow() -> Result<String, OpenRouterOAuthError> {
    let login = start_openrouter_browser_login().await?;
    webbrowser::open(&login.authorize_url).map_err(|_| OpenRouterOAuthError::Browser)?;
    complete_openrouter_browser_login(login).await
}

pub async fn start_openrouter_browser_login() -> Result<OpenRouterBrowserLogin, OpenRouterOAuthError>
{
    let callback_nonce = random_token(32);
    let callback_path = format!("{CALLBACK_PATH_PREFIX}{callback_nonce}");
    let bound = bind_loopback(0, &callback_path)
        .await
        .map_err(|error| match error {
            LoopbackBindError::Bind(error) => OpenRouterOAuthError::Bind(error),
            LoopbackBindError::LocalAddress(error) => OpenRouterOAuthError::LocalAddress(error),
        })?;
    let request = build_oauth_request(&bound.callback_url, random_token(64));
    Ok(OpenRouterBrowserLogin {
        authorize_url: request.authorize_url,
        listener: bound.listener,
        callback_path,
        verifier: request.verifier,
    })
}

pub async fn complete_openrouter_browser_login(
    login: OpenRouterBrowserLogin,
) -> Result<String, OpenRouterOAuthError> {
    let client = http_client()?;
    let code = timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(&login.listener, &login.callback_path),
    )
    .await
    .map_err(|_| OpenRouterOAuthError::Timeout)??;
    exchange_code(&client, &code, &login.verifier).await
}

fn build_oauth_request(callback_url: &str, verifier: String) -> OpenRouterOAuthRequest {
    build_oauth_request_with_endpoint(AUTHORIZE_URL, callback_url, verifier)
}

fn build_oauth_request_with_endpoint(
    authorize_endpoint: &str,
    callback_url: &str,
    verifier: String,
) -> OpenRouterOAuthRequest {
    let challenge = pkce_challenge(&verifier);
    let mut url = Url::parse(authorize_endpoint).expect("OpenRouter authorize URL must be valid");
    url.query_pairs_mut()
        .append_pair("callback_url", callback_url)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    OpenRouterOAuthRequest {
        authorize_url: url.to_string(),
        verifier,
    }
}

fn http_client() -> Result<reqwest::Client, OpenRouterOAuthError> {
    crate::reqwest_client_builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(crate::rho_user_agent())
        .build()
        .map_err(OpenRouterOAuthError::from)
}

async fn wait_for_callback(
    listener: &TcpListener,
    expected_path: &str,
) -> Result<String, OpenRouterOAuthError> {
    wait_for_callback_with_read_timeout(listener, expected_path, CALLBACK_READ_TIMEOUT).await
}

async fn wait_for_callback_with_read_timeout(
    listener: &TcpListener,
    expected_path: &str,
    read_timeout: Duration,
) -> Result<String, OpenRouterOAuthError> {
    const BODIES: ResponseBodies<'static> = ResponseBodies {
        success: "Authorization received. Return to Rho to finish OpenRouter login.",
        failure: "OpenRouter login failed. Return to Rho for details and try again.",
        ignored: "This is not the OpenRouter callback.",
    };
    loop {
        let (mut stream, request) = accept_request(listener, read_timeout)
            .await
            .map_err(OpenRouterOAuthError::Accept)?;
        let Some(request) = request else {
            let _ = write_response(&mut stream, ResponseKind::Ignored, BODIES).await;
            continue;
        };
        match parse_callback_http_request(&request, expected_path) {
            CallbackParse::Code(code) => {
                let _ = write_response(&mut stream, ResponseKind::Success, BODIES).await;
                return Ok(code);
            }
            CallbackParse::Denied(error) => {
                let _ = write_response(&mut stream, ResponseKind::Failure, BODIES).await;
                return Err(OpenRouterOAuthError::OAuthDenied(error));
            }
            CallbackParse::Ignored => {
                let _ = write_response(&mut stream, ResponseKind::Ignored, BODIES).await;
            }
            CallbackParse::Invalid => {
                let _ = write_response(&mut stream, ResponseKind::Failure, BODIES).await;
                return Err(OpenRouterOAuthError::InvalidCallback);
            }
        }
    }
}

fn parse_callback_http_request(request: &str, expected_path: &str) -> CallbackParse {
    let request_line = request.lines().next().unwrap_or_default().trim();
    if request_line.is_empty() {
        return CallbackParse::Ignored;
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if !method.eq_ignore_ascii_case("GET") || target.is_empty() {
        return CallbackParse::Ignored;
    }

    let url = match Url::parse(&format!("http://localhost{target}")) {
        Ok(url) => url,
        Err(_) => return CallbackParse::Ignored,
    };
    if url.path() != expected_path {
        return CallbackParse::Ignored;
    }

    let params = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    if let Some(error) = params.get("error") {
        return CallbackParse::Denied(
            params
                .get("error_description")
                .cloned()
                .unwrap_or_else(|| error.clone()),
        );
    }
    match params.get("code").cloned() {
        Some(code) if !code.is_empty() => CallbackParse::Code(code),
        _ => CallbackParse::Invalid,
    }
}

async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> Result<String, OpenRouterOAuthError> {
    exchange_code_with_endpoint(client, code, verifier, KEY_EXCHANGE_URL).await
}

async fn exchange_code_with_endpoint(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
    endpoint: &str,
) -> Result<String, OpenRouterOAuthError> {
    let response = client
        .post(endpoint)
        .json(&KeyExchangeRequest {
            code,
            code_verifier: verifier,
            code_challenge_method: "S256",
        })
        .send()
        .await
        .map_err(OpenRouterOAuthError::from)?;
    let status = response.status();
    if !status.is_success() {
        return Err(OpenRouterOAuthError::ExchangeStatus(status));
    }
    let response = response
        .json::<KeyExchangeResponse>()
        .await
        .map_err(|error| {
            OpenRouterOAuthError::InvalidResponse(TransportError::from_reqwest(error))
        })?;
    response
        .key
        .filter(|key| !key.trim().is_empty())
        .ok_or(OpenRouterOAuthError::MissingKey)
}

#[cfg(test)]
#[path = "openrouter_oauth_tests.rs"]
mod tests;
