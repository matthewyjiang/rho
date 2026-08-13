//! Anthropic browser OAuth for usage-credits billing.
//!
//! This is not Claude Code subscription auth. Third-party use of a Claude
//! login spends extra usage / usage credits, not the included plan allowance.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::credentials::AnthropicTokens;

use super::loopback::{pkce_challenge, random_token};

pub(crate) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub(crate) const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPE: &str = "org:create_api_key user:profile user:inference";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// `anthropic-beta` header value required on OAuth-authorized API requests.
pub(crate) const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

#[derive(Clone)]
pub struct AnthropicOAuthRequest {
    pub authorize_url: String,
    redirect_uri: String,
    state: String,
    verifier: String,
}

impl std::fmt::Debug for AnthropicOAuthRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AnthropicOAuthRequest")
            .field("authorize_url", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnthropicOAuthError {
    #[error("Anthropic OAuth request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Anthropic OAuth token response was missing {0}")]
    MissingToken(&'static str),
    #[error("Anthropic OAuth was denied or failed: {0}")]
    OAuthDenied(String),
    #[error("Anthropic OAuth authorization code is empty")]
    EmptyCode,
}

#[derive(Serialize)]
struct TokenExchangeRequest<'a> {
    grant_type: &'static str,
    client_id: &'a str,
    code: &'a str,
    redirect_uri: &'a str,
    code_verifier: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Builds the Anthropic OAuth authorize request without opening a browser.
pub fn build_oauth_request() -> AnthropicOAuthRequest {
    build_oauth_request_with_values(random_token(32), random_token(64))
}

fn build_oauth_request_with_values(state: String, verifier: String) -> AnthropicOAuthRequest {
    let challenge = pkce_challenge(&verifier);
    let mut url = Url::parse(AUTHORIZE_URL).expect("Anthropic authorize URL must be valid");
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    AnthropicOAuthRequest {
        authorize_url: url.to_string(),
        redirect_uri: REDIRECT_URI.into(),
        state,
        verifier,
    }
}

pub async fn complete_anthropic_oauth(
    request: AnthropicOAuthRequest,
    raw_code: &str,
) -> Result<AnthropicTokens, AnthropicOAuthError> {
    complete_anthropic_oauth_with_endpoint(&http_client()?, request, raw_code, TOKEN_URL).await
}

async fn complete_anthropic_oauth_with_endpoint(
    client: &reqwest::Client,
    request: AnthropicOAuthRequest,
    raw_code: &str,
    endpoint: &str,
) -> Result<AnthropicTokens, AnthropicOAuthError> {
    let parsed = parse_authorization_code(raw_code).ok_or(AnthropicOAuthError::EmptyCode)?;
    let state = parsed.state.as_deref().unwrap_or(request.state.as_str());
    let response = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("User-Agent", "anthropic")
        .json(&TokenExchangeRequest {
            grant_type: "authorization_code",
            client_id: CLIENT_ID,
            code: &parsed.code,
            redirect_uri: &request.redirect_uri,
            code_verifier: &request.verifier,
            state: Some(state),
        })
        .send()
        .await?
        .error_for_status()?
        .json::<TokenResponse>()
        .await?;
    tokens_from_response(response)
}

struct ParsedAuthorizationCode {
    code: String,
    state: Option<String>,
}

fn parse_authorization_code(raw: &str) -> Option<ParsedAuthorizationCode> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (code, state) = match raw.split_once('#') {
        Some((code, state)) => (code, Some(state.to_string())),
        None => (raw, None),
    };
    let code = code.trim();
    if code.is_empty() {
        return None;
    }
    Some(ParsedAuthorizationCode {
        code: code.to_string(),
        state: state.filter(|value| !value.trim().is_empty()),
    })
}

fn http_client() -> Result<reqwest::Client, AnthropicOAuthError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("anthropic")
        .build()
        .map_err(AnthropicOAuthError::Request)
}

fn tokens_from_response(response: TokenResponse) -> Result<AnthropicTokens, AnthropicOAuthError> {
    if let Some(error) = response.error {
        return Err(AnthropicOAuthError::OAuthDenied(
            response.error_description.unwrap_or(error),
        ));
    }
    let access_token = response
        .access_token
        .ok_or(AnthropicOAuthError::MissingToken("access_token"))?;
    let expires_at_unix = response.expires_in.and_then(|expires_in| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|now| i64::try_from(now.as_secs().saturating_add(expires_in)).ok())
    });
    Ok(AnthropicTokens {
        access_token,
        refresh_token: response.refresh_token,
        expires_at_unix,
    })
}

#[cfg(test)]
#[path = "anthropic_oauth_tests.rs"]
mod tests;
