use std::{collections::HashMap, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};
use url::Url;

use crate::credentials::CodexTokens;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct OAuthRequest {
    pub authorize_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub verifier: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackOutcome {
    Code(String),
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CodexOAuthError {
    #[error("could not bind local OAuth callback listener: {0}")]
    Bind(std::io::Error),
    #[error("could not open browser for Codex OAuth: {0}")]
    Browser(String),
    #[error("timed out waiting for Codex OAuth browser callback")]
    Timeout,
    #[error("could not read OAuth callback: {0}")]
    CallbackIo(std::io::Error),
    #[error("OAuth callback was invalid: {0}")]
    InvalidCallback(String),
    #[error("OAuth was denied or failed: {0}")]
    OAuthDenied(String),
    #[error("token exchange failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("token response was missing {0}")]
    MissingToken(&'static str),
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<IdTokenAuthClaims>,
}

#[derive(Debug, Deserialize)]
struct IdTokenAuthClaims {
    chatgpt_account_id: Option<String>,
}

pub async fn run_codex_oauth_flow() -> Result<CodexTokens, CodexOAuthError> {
    let listener = TcpListener::bind((CALLBACK_HOST, CALLBACK_PORT))
        .await
        .map_err(CodexOAuthError::Bind)?;
    let request = build_oauth_request();

    webbrowser::open(&request.authorize_url)
        .map_err(|err| CodexOAuthError::Browser(err.to_string()))?;

    let code = match timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(&listener, &request.state),
    )
    .await
    {
        Ok(Ok(CallbackOutcome::Code(code))) => code,
        Ok(Ok(CallbackOutcome::Error(error))) => return Err(CodexOAuthError::OAuthDenied(error)),
        Ok(Err(err)) => return Err(err),
        Err(_) => return Err(CodexOAuthError::Timeout),
    };

    exchange_code(
        &reqwest::Client::new(),
        &code,
        &request.redirect_uri,
        &request.verifier,
    )
    .await
}

pub fn build_oauth_request() -> OAuthRequest {
    let redirect_uri = format!("http://{CALLBACK_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}");
    build_oauth_request_with_values(random_token(32), random_token(64), redirect_uri)
}

fn build_oauth_request_with_values(
    state: String,
    verifier: String,
    redirect_uri: String,
) -> OAuthRequest {
    let challenge = pkce_challenge(&verifier);
    let mut url = Url::parse(AUTHORIZE_URL).expect("authorize URL must be valid");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "codex_cli_rs");

    OAuthRequest {
        authorize_url: url.to_string(),
        redirect_uri,
        state,
        verifier,
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
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

async fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<CallbackOutcome, CodexOAuthError> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(CodexOAuthError::CallbackIo)?;
    let mut buffer = vec![0_u8; 8192];
    let len = stream
        .read(&mut buffer)
        .await
        .map_err(CodexOAuthError::CallbackIo)?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let first_line = request.lines().next().unwrap_or_default();
    let outcome = parse_callback_request_line(first_line, expected_state);
    let body = match &outcome {
        Ok(CallbackOutcome::Code(_)) => "Codex login complete. You can return to Rho.",
        Ok(CallbackOutcome::Error(_)) | Err(_) => {
            "Codex login failed. You can return to Rho for details."
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
) -> Result<CallbackOutcome, CodexOAuthError> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" || target.is_empty() {
        return Err(CodexOAuthError::InvalidCallback(
            "expected GET callback request".into(),
        ));
    }
    let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|err| {
        CodexOAuthError::InvalidCallback(format!("callback URL could not be parsed: {err}"))
    })?;
    if url.path() != CALLBACK_PATH {
        return Err(CodexOAuthError::InvalidCallback(format!(
            "callback path was not {CALLBACK_PATH}"
        )));
    }
    let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
    let state = query
        .get("state")
        .ok_or_else(|| CodexOAuthError::InvalidCallback("callback was missing state".into()))?;
    if state != expected_state {
        return Err(CodexOAuthError::InvalidCallback(
            "callback state did not match".into(),
        ));
    }
    if let Some(error) = query.get("error") {
        return Ok(CallbackOutcome::Error(error.clone()));
    }
    let code = query
        .get("code")
        .ok_or_else(|| CodexOAuthError::InvalidCallback("callback was missing code".into()))?;
    Ok(CallbackOutcome::Code(code.clone()))
}

async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<CodexTokens, CodexOAuthError> {
    let response: TokenResponse = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let account_id = response
        .id_token
        .as_deref()
        .and_then(account_id_from_id_token)
        .or(response.account_id);

    Ok(CodexTokens {
        access_token: response
            .access_token
            .ok_or(CodexOAuthError::MissingToken("access_token"))?,
        refresh_token: Some(
            response
                .refresh_token
                .ok_or(CodexOAuthError::MissingToken("refresh_token"))?,
        ),
        id_token: response.id_token,
        account_id,
    })
}

fn account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: IdTokenClaims = serde_json::from_slice(&decoded).ok()?;
    claims.auth?.chatgpt_account_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_contains_pkce_and_loopback_redirect() {
        let request = build_oauth_request_with_values(
            "state123".into(),
            "verifier123".into(),
            "http://127.0.0.1:1455/auth/callback".into(),
        );
        let url = Url::parse(&request.authorize_url).unwrap();
        let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();

        assert_eq!(url.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
        assert_eq!(query.get("client_id").unwrap(), CLIENT_ID);
        assert_eq!(query.get("response_type").unwrap(), "code");
        assert_eq!(query.get("redirect_uri").unwrap(), &request.redirect_uri);
        assert_eq!(query.get("state").unwrap(), "state123");
        assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(query.get("id_token_add_organizations").unwrap(), "true");
        assert_eq!(query.get("codex_cli_simplified_flow").unwrap(), "true");
        assert_eq!(query.get("originator").unwrap(), "codex_cli_rs");
        assert!(query.get("scope").unwrap().contains("api.connectors.read"));
        assert_eq!(
            query.get("code_challenge").unwrap(),
            &pkce_challenge("verifier123")
        );
    }

    #[test]
    fn extracts_account_id_from_id_token_claims() {
        let claims = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123"
            }
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let id_token = format!("header.{payload}.signature");

        assert_eq!(
            account_id_from_id_token(&id_token),
            Some("account-123".into())
        );
    }

    #[test]
    fn callback_parser_accepts_matching_state_and_code() {
        assert_eq!(
            parse_callback_request_line("GET /auth/callback?code=abc&state=ok HTTP/1.1", "ok")
                .unwrap(),
            CallbackOutcome::Code("abc".into())
        );
    }

    #[test]
    fn callback_parser_rejects_state_mismatch() {
        let err = parse_callback_request_line(
            "GET /auth/callback?code=abc&state=bad HTTP/1.1",
            "expected",
        )
        .unwrap_err();

        assert!(err.to_string().contains("state did not match"));
    }

    #[test]
    fn callback_parser_reports_oauth_error() {
        assert_eq!(
            parse_callback_request_line(
                "GET /auth/callback?error=access_denied&state=ok HTTP/1.1",
                "ok"
            )
            .unwrap(),
            CallbackOutcome::Error("access_denied".into())
        );
    }
}
