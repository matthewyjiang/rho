use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, time::timeout};
use url::Url;

use super::loopback::{
    accept_request, bind_ipv4, callback_url, pkce_challenge, random_token, write_response,
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
    Request(#[source] reqwest::Error),
    #[error("the OpenRouter OAuth key endpoint returned HTTP {0}")]
    ExchangeStatus(reqwest::StatusCode),
    #[error("the OpenRouter OAuth key response was invalid: {0}")]
    InvalidResponse(#[source] reqwest::Error),
    #[error("the OpenRouter OAuth key response did not include a key")]
    MissingKey,
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

pub async fn run_openrouter_oauth_flow() -> Result<String, OpenRouterOAuthError> {
    let client = http_client()?;
    let listener = bind_ipv4(0).await.map_err(OpenRouterOAuthError::Bind)?;
    let callback_nonce = random_token(32);
    let callback_path = format!("{CALLBACK_PATH_PREFIX}{callback_nonce}");
    let callback_url =
        callback_url(&listener, &callback_path).map_err(OpenRouterOAuthError::LocalAddress)?;
    let request = build_oauth_request(&callback_url, random_token(64));

    webbrowser::open(&request.authorize_url).map_err(|_| OpenRouterOAuthError::Browser)?;

    let code = timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(&listener, &callback_path),
    )
    .await
    .map_err(|_| OpenRouterOAuthError::Timeout)??;
    exchange_code(&client, &code, &request.verifier).await
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
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(crate::rho_user_agent())
        .build()
        .map_err(OpenRouterOAuthError::Request)
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
        .map_err(OpenRouterOAuthError::Request)?;
    let status = response.status();
    if !status.is_success() {
        return Err(OpenRouterOAuthError::ExchangeStatus(status));
    }
    let response = response
        .json::<KeyExchangeResponse>()
        .await
        .map_err(OpenRouterOAuthError::InvalidResponse)?;
    response
        .key
        .filter(|key| !key.trim().is_empty())
        .ok_or(OpenRouterOAuthError::MissingKey)
}

#[cfg(test)]
#[path = "openrouter_oauth_tests.rs"]
mod tests;
