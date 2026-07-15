use std::{collections::HashMap, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{sleep, timeout, Instant},
};
use url::Url;

use crate::credentials::CodexTokens;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER_URL: &str = "https://auth.openai.com";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CALLBACK_BIND_HOST_IPV4: &str = "127.0.0.1";
const CALLBACK_BIND_HOST_IPV6: &str = "::1";
const CALLBACK_REDIRECT_HOST: &str = "localhost";
const CALLBACK_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
const DEVICE_CODE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DEFAULT_DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct OAuthRequest {
    pub authorize_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub verifier: String,
}

#[derive(Clone)]
pub struct CodexDeviceLogin {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: Duration,
    device_auth_id: String,
    interval: Duration,
}

#[derive(Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    Code(String),
    Error(String),
}

impl std::fmt::Debug for OAuthRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthRequest")
            .field("authorize_url", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for CodexDeviceLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexDeviceLogin")
            .field("user_code", &"[REDACTED]")
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("device_auth_id", &"[REDACTED]")
            .field("interval", &self.interval)
            .finish()
    }
}

impl std::fmt::Debug for CallbackOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code(_) => formatter.write_str("Code([REDACTED])"),
            Self::Error(_) => formatter.write_str("Error([REDACTED])"),
        }
    }
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
    #[error("Codex device login setup failed: {0}")]
    DeviceSetup(String),
    #[error("timed out waiting for Codex device login")]
    DeviceTimeout,
    #[error("token exchange failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("token response was missing {0}")]
    MissingToken(&'static str),
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct IdTokenClaims {
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<IdTokenAuthClaims>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChatGptPlan {
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    Team,
    SelfServeBusinessUsageBased,
    Business,
    EnterpriseCbpUsageBased,
    Enterprise,
    Edu,
    Unknown,
}

impl ChatGptPlan {
    fn from_claim(claim: &str) -> Self {
        match claim.trim().to_ascii_lowercase().as_str() {
            "free" => Self::Free,
            "go" => Self::Go,
            "plus" => Self::Plus,
            "pro" => Self::Pro,
            "prolite" => Self::ProLite,
            "team" => Self::Team,
            "self_serve_business_usage_based" => Self::SelfServeBusinessUsageBased,
            "business" => Self::Business,
            "enterprise_cbp_usage_based" => Self::EnterpriseCbpUsageBased,
            "enterprise" | "hc" => Self::Enterprise,
            "education" | "edu" => Self::Edu,
            _ => Self::Unknown,
        }
    }
}

#[derive(Deserialize)]
struct IdTokenAuthClaims {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
}

#[derive(Deserialize)]
struct DeviceUserCodeResponse {
    device_auth_id: Option<String>,
    #[serde(alias = "usercode")]
    user_code: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_interval_seconds")]
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    authorization_code: Option<String>,
    code_verifier: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Serialize)]
struct DeviceUserCodeRequest<'a> {
    client_id: &'a str,
}

#[derive(Serialize)]
struct DeviceTokenRequest<'a> {
    device_auth_id: &'a str,
    user_code: &'a str,
}

pub async fn run_codex_oauth_flow() -> Result<CodexTokens, CodexOAuthError> {
    let client = http_client()?;
    let listeners = bind_callback_listeners().await?;
    let request = build_oauth_request();

    webbrowser::open(&request.authorize_url)
        .map_err(|err| CodexOAuthError::Browser(err.to_string()))?;

    let code = match timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(&listeners, &request.state),
    )
    .await
    {
        Ok(Ok(CallbackOutcome::Code(code))) => code,
        Ok(Ok(CallbackOutcome::Error(error))) => return Err(CodexOAuthError::OAuthDenied(error)),
        Ok(Err(err)) => return Err(err),
        Err(_) => return Err(CodexOAuthError::Timeout),
    };

    exchange_code(&client, &code, &request.redirect_uri, &request.verifier).await
}

pub async fn start_codex_device_login() -> Result<CodexDeviceLogin, CodexOAuthError> {
    start_codex_device_login_with_endpoint(&http_client()?, &device_user_code_url()).await
}

pub async fn complete_codex_device_login(
    login: CodexDeviceLogin,
) -> Result<CodexTokens, CodexOAuthError> {
    complete_codex_device_login_with_endpoints(
        &http_client()?,
        login,
        &device_token_url(),
        &device_redirect_uri(),
    )
    .await
}

pub fn build_oauth_request() -> OAuthRequest {
    let redirect_uri = format!("http://{CALLBACK_REDIRECT_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}");
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

fn http_client() -> Result<reqwest::Client, CodexOAuthError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(CodexOAuthError::Request)
}

fn device_user_code_url() -> String {
    format!("{ISSUER_URL}/api/accounts/deviceauth/usercode")
}

fn device_token_url() -> String {
    format!("{ISSUER_URL}/api/accounts/deviceauth/token")
}

fn device_redirect_uri() -> String {
    format!("{ISSUER_URL}/deviceauth/callback")
}

struct CallbackListeners {
    ipv4: Option<TcpListener>,
    ipv6: Option<TcpListener>,
}

async fn bind_callback_listeners() -> Result<CallbackListeners, CodexOAuthError> {
    let ipv4 = TcpListener::bind((CALLBACK_BIND_HOST_IPV4, CALLBACK_PORT)).await;
    let ipv6 = TcpListener::bind((CALLBACK_BIND_HOST_IPV6, CALLBACK_PORT)).await;

    match (ipv4, ipv6) {
        (Ok(ipv4), Ok(ipv6)) => Ok(CallbackListeners {
            ipv4: Some(ipv4),
            ipv6: Some(ipv6),
        }),
        (Ok(ipv4), Err(_)) => Ok(CallbackListeners {
            ipv4: Some(ipv4),
            ipv6: None,
        }),
        (Err(_), Ok(ipv6)) => Ok(CallbackListeners {
            ipv4: None,
            ipv6: Some(ipv6),
        }),
        (Err(ipv4), Err(_)) => Err(CodexOAuthError::Bind(ipv4)),
    }
}

async fn wait_for_callback(
    listeners: &CallbackListeners,
    expected_state: &str,
) -> Result<CallbackOutcome, CodexOAuthError> {
    let mut stream = accept_callback(listeners).await?;
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

async fn accept_callback(listeners: &CallbackListeners) -> Result<TcpStream, CodexOAuthError> {
    match (&listeners.ipv4, &listeners.ipv6) {
        (Some(ipv4), Some(ipv6)) => {
            tokio::select! {
                result = ipv4.accept() => result,
                result = ipv6.accept() => result,
            }
        }
        (Some(ipv4), None) => ipv4.accept().await,
        (None, Some(ipv6)) => ipv6.accept().await,
        (None, None) => {
            return Err(CodexOAuthError::CallbackIo(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "no OAuth callback listeners were available",
            )))
        }
    }
    .map(|(stream, _)| stream)
    .map_err(CodexOAuthError::CallbackIo)
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

async fn start_codex_device_login_with_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
) -> Result<CodexDeviceLogin, CodexOAuthError> {
    let response: DeviceUserCodeResponse = client
        .post(endpoint)
        .json(&DeviceUserCodeRequest {
            client_id: CLIENT_ID,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(error) = response.error {
        return Err(CodexOAuthError::DeviceSetup(
            response.error_description.unwrap_or(error),
        ));
    }

    Ok(CodexDeviceLogin {
        device_auth_id: required(response.device_auth_id, "device_auth_id")?,
        user_code: required(response.user_code, "user_code")?,
        verification_uri: format!("{ISSUER_URL}/codex/device"),
        expires_in: DEVICE_CODE_TIMEOUT,
        interval: response
            .interval
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL),
    })
}

async fn complete_codex_device_login_with_endpoints(
    client: &reqwest::Client,
    login: CodexDeviceLogin,
    token_endpoint: &str,
    redirect_uri: &str,
) -> Result<CodexTokens, CodexOAuthError> {
    complete_codex_device_login_with_exchange_endpoint(
        client,
        login,
        token_endpoint,
        redirect_uri,
        TOKEN_URL,
    )
    .await
}

async fn complete_codex_device_login_with_exchange_endpoint(
    client: &reqwest::Client,
    login: CodexDeviceLogin,
    token_endpoint: &str,
    redirect_uri: &str,
    exchange_endpoint: &str,
) -> Result<CodexTokens, CodexOAuthError> {
    let deadline = Instant::now() + login.expires_in;

    loop {
        if Instant::now() >= deadline {
            return Err(CodexOAuthError::DeviceTimeout);
        }

        let response = client
            .post(token_endpoint)
            .json(&DeviceTokenRequest {
                device_auth_id: &login.device_auth_id,
                user_code: &login.user_code,
            })
            .send()
            .await?;

        if response.status().is_success() {
            let response: DeviceTokenResponse = response.json().await?;
            if let Some(error) = response.error {
                return Err(CodexOAuthError::OAuthDenied(
                    response.error_description.unwrap_or(error),
                ));
            }
            let authorization_code = required(response.authorization_code, "authorization_code")?;
            let code_verifier = required(response.code_verifier, "code_verifier")?;
            return exchange_code_with_endpoint(
                client,
                exchange_endpoint,
                &authorization_code,
                redirect_uri,
                &code_verifier,
            )
            .await;
        }

        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            let remaining = deadline.saturating_duration_since(Instant::now());
            sleep(login.interval.min(remaining)).await;
            continue;
        }

        response.error_for_status()?;
    }
}

fn required<T>(value: Option<T>, field: &'static str) -> Result<T, CodexOAuthError> {
    value.ok_or(CodexOAuthError::MissingToken(field))
}

fn deserialize_optional_interval_seconds<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Interval {
        String(String),
        Number(u64),
    }

    let value = Option::<Interval>::deserialize(deserializer)?;
    match value {
        Some(Interval::String(value)) => value
            .trim()
            .parse()
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(Interval::Number(value)) => Ok(Some(value)),
        None => Ok(None),
    }
}

async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<CodexTokens, CodexOAuthError> {
    exchange_code_with_endpoint(client, TOKEN_URL, code, redirect_uri, verifier).await
}

async fn exchange_code_with_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<CodexTokens, CodexOAuthError> {
    let response: TokenResponse = client
        .post(endpoint)
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

pub(crate) fn chatgpt_plan_from_id_token(id_token: &str) -> ChatGptPlan {
    let payload = id_token.split('.').nth(1);
    let Some(payload) = payload else {
        return ChatGptPlan::Unknown;
    };
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(payload) else {
        return ChatGptPlan::Unknown;
    };
    let Ok(claims) = serde_json::from_slice::<IdTokenClaims>(&decoded) else {
        return ChatGptPlan::Unknown;
    };
    let Some(plan_type) = claims.auth.and_then(|auth| auth.chatgpt_plan_type) else {
        return ChatGptPlan::Unknown;
    };
    ChatGptPlan::from_claim(&plan_type)
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
    fn oauth_debug_redacts_codes_and_pkce_secrets() {
        let request = build_oauth_request_with_values(
            "oauth-state-secret".into(),
            "pkce-verifier-secret".into(),
            "http://localhost:1455/auth/callback".into(),
        );
        let request_debug = format!("{request:?}");
        assert!(!request_debug.contains("oauth-state-secret"));
        assert!(!request_debug.contains("pkce-verifier-secret"));
        assert!(
            !format!("{:?}", CallbackOutcome::Code("oauth-code-secret".into()))
                .contains("oauth-code-secret")
        );
    }

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn authorize_url_contains_pkce_and_loopback_redirect() {
        let request = build_oauth_request_with_values(
            "state123".into(),
            "verifier123".into(),
            "http://localhost:1455/auth/callback".into(),
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
    fn extracts_plan_from_id_token_claims() {
        for (plan, expected) in [
            ("free", ChatGptPlan::Free),
            ("go", ChatGptPlan::Go),
            ("plus", ChatGptPlan::Plus),
            ("pro", ChatGptPlan::Pro),
            ("prolite", ChatGptPlan::ProLite),
            ("team", ChatGptPlan::Team),
            (
                "self_serve_business_usage_based",
                ChatGptPlan::SelfServeBusinessUsageBased,
            ),
            ("business", ChatGptPlan::Business),
            (
                "enterprise_cbp_usage_based",
                ChatGptPlan::EnterpriseCbpUsageBased,
            ),
            ("hc", ChatGptPlan::Enterprise),
            ("education", ChatGptPlan::Edu),
            ("unexpected", ChatGptPlan::Unknown),
        ] {
            let claims = serde_json::json!({
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": plan
                }
            });
            let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
            let id_token = format!("header.{payload}.signature");

            assert_eq!(
                chatgpt_plan_from_id_token(&id_token),
                expected,
                "plan: {plan}"
            );
        }
    }

    #[test]
    fn unknown_plan_is_returned_for_invalid_or_missing_id_token_claims() {
        assert_eq!(
            chatgpt_plan_from_id_token("not-a-jwt"),
            ChatGptPlan::Unknown
        );

        let claims = serde_json::json!({"sub": "user-123"});
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let id_token = format!("header.{payload}.signature");

        assert_eq!(chatgpt_plan_from_id_token(&id_token), ChatGptPlan::Unknown);
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

    #[tokio::test]
    async fn codex_device_login_posts_client_id_and_parses_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let len = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.contains("POST / HTTP/1.1"));
            assert!(request.contains("application/json"));
            assert!(request.contains("app_EMoamEEZ73f0CkXaXp7hrann"));
            let body = r#"{"device_auth_id":"device","user_code":"ABCD-EFGH","interval":"1"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let login = start_codex_device_login_with_endpoint(&reqwest::Client::new(), &endpoint)
            .await
            .unwrap();

        assert_eq!(login.user_code, "ABCD-EFGH");
        assert_eq!(
            login.verification_uri,
            "https://auth.openai.com/codex/device"
        );
        assert_eq!(login.expires_in, DEVICE_CODE_TIMEOUT);
        assert_eq!(login.interval, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn codex_device_login_exchanges_authorization_code_for_tokens() {
        let token_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let token_endpoint = format!("http://{}", token_listener.local_addr().unwrap());
        let exchange_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let exchange_endpoint = format!("http://{}", exchange_listener.local_addr().unwrap());

        tokio::spawn(async move {
            let (mut stream, _) = token_listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let len = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.contains("device_auth_id"));
            assert!(request.contains("ABCD-EFGH"));
            let body = r#"{"authorization_code":"auth-code","code_challenge":"challenge","code_verifier":"verifier"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        tokio::spawn(async move {
            let (mut stream, _) = exchange_listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let len = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.contains("code=auth-code"));
            assert!(request
                .contains("redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback"));
            assert!(request.contains("code_verifier=verifier"));
            let body = r#"{"access_token":"access","refresh_token":"refresh","id_token":"id"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let tokens = complete_codex_device_login_with_exchange_endpoint(
            &reqwest::Client::new(),
            CodexDeviceLogin {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://auth.openai.com/codex/device".into(),
                expires_in: Duration::from_secs(10),
                device_auth_id: "device".into(),
                interval: Duration::from_millis(1),
            },
            &token_endpoint,
            "https://auth.openai.com/deviceauth/callback",
            &exchange_endpoint,
        )
        .await
        .unwrap();

        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(tokens.id_token.as_deref(), Some("id"));
    }
}
