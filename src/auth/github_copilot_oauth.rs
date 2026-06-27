use std::{collections::HashMap, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use url::Url;

use crate::credentials::GitHubCopilotTokens;

pub const GITHUB_COPILOT_CLIENT_ID_ENV: &str = "RHO_GITHUB_COPILOT_CLIENT_ID";
pub const GITHUB_COPILOT_CLIENT_SECRET_ENV: &str = "RHO_GITHUB_COPILOT_CLIENT_SECRET";
const AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEFAULT_SCOPE: &str = "read:user";
const CALLBACK_BIND_HOST_IPV4: &str = "127.0.0.1";
const CALLBACK_BIND_HOST_IPV6: &str = "::1";
const CALLBACK_REDIRECT_HOST: &str = "localhost";
const CALLBACK_PORT: u16 = 1456;
const CALLBACK_PATH: &str = "/auth/github-copilot/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
const USER_AGENT: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct GitHubCopilotOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Clone, Debug)]
pub struct GitHubCopilotOAuthRequest {
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
pub enum GitHubCopilotOAuthError {
    #[error("GitHub Copilot browser OAuth requires an app-owned GitHub OAuth client id and secret; set {GITHUB_COPILOT_CLIENT_ID_ENV} and retry /login github-copilot")]
    MissingClientId,
    #[error("GitHub Copilot browser OAuth requires an app-owned GitHub OAuth client secret; set {GITHUB_COPILOT_CLIENT_SECRET_ENV} and retry /login github-copilot")]
    MissingClientSecret,
    #[error("could not bind local GitHub Copilot OAuth callback listener: {0}")]
    Bind(std::io::Error),
    #[error("could not open browser for GitHub Copilot OAuth: {0}")]
    Browser(String),
    #[error("timed out waiting for GitHub Copilot OAuth browser callback")]
    Timeout,
    #[error("could not read GitHub Copilot OAuth callback: {0}")]
    CallbackIo(std::io::Error),
    #[error("GitHub Copilot OAuth callback was invalid: {0}")]
    InvalidCallback(String),
    #[error("GitHub Copilot OAuth was denied or failed: {0}")]
    OAuthDenied(String),
    #[error("GitHub Copilot token exchange failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("GitHub Copilot token response was missing {0}")]
    MissingToken(&'static str),
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub fn github_copilot_oauth_config_from_env(
) -> Result<GitHubCopilotOAuthConfig, GitHubCopilotOAuthError> {
    let client_id = required_env(
        GITHUB_COPILOT_CLIENT_ID_ENV,
        GitHubCopilotOAuthError::MissingClientId,
    )?;
    let client_secret = required_env(
        GITHUB_COPILOT_CLIENT_SECRET_ENV,
        GitHubCopilotOAuthError::MissingClientSecret,
    )?;
    Ok(GitHubCopilotOAuthConfig {
        client_id,
        client_secret,
    })
}

fn required_env(
    key: &str,
    missing: GitHubCopilotOAuthError,
) -> Result<String, GitHubCopilotOAuthError> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(_) => Err(missing),
    }
}

pub async fn run_github_copilot_oauth_flow(
    config: GitHubCopilotOAuthConfig,
) -> Result<GitHubCopilotTokens, GitHubCopilotOAuthError> {
    let listeners = bind_callback_listeners().await?;
    let request = build_oauth_request(config.client_id.clone());

    webbrowser::open(&request.authorize_url)
        .map_err(|err| GitHubCopilotOAuthError::Browser(err.to_string()))?;

    let code = match timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(&listeners, &request.state),
    )
    .await
    {
        Ok(Ok(CallbackOutcome::Code(code))) => code,
        Ok(Ok(CallbackOutcome::Error(error))) => {
            return Err(GitHubCopilotOAuthError::OAuthDenied(error));
        }
        Ok(Err(err)) => return Err(err),
        Err(_) => return Err(GitHubCopilotOAuthError::Timeout),
    };

    exchange_code(
        &reqwest::Client::new(),
        &code,
        &request.redirect_uri,
        &request.verifier,
        &config,
    )
    .await
}

fn build_oauth_request(client_id: String) -> GitHubCopilotOAuthRequest {
    let redirect_uri = format!("http://{CALLBACK_REDIRECT_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}");
    build_oauth_request_with_values(client_id, random_token(32), random_token(64), redirect_uri)
}

fn build_oauth_request_with_values(
    client_id: String,
    state: String,
    verifier: String,
    redirect_uri: String,
) -> GitHubCopilotOAuthRequest {
    let challenge = pkce_challenge(&verifier);
    let mut url = Url::parse(AUTHORIZE_URL).expect("authorize URL must be valid");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", DEFAULT_SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");

    GitHubCopilotOAuthRequest {
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

struct CallbackListeners {
    ipv4: Option<TcpListener>,
    ipv6: Option<TcpListener>,
}

async fn bind_callback_listeners() -> Result<CallbackListeners, GitHubCopilotOAuthError> {
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
        (Err(ipv4), Err(_)) => Err(GitHubCopilotOAuthError::Bind(ipv4)),
    }
}

async fn wait_for_callback(
    listeners: &CallbackListeners,
    expected_state: &str,
) -> Result<CallbackOutcome, GitHubCopilotOAuthError> {
    let mut stream = accept_callback(listeners).await?;
    let mut buffer = vec![0_u8; 8192];
    let len = stream
        .read(&mut buffer)
        .await
        .map_err(GitHubCopilotOAuthError::CallbackIo)?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let first_line = request.lines().next().unwrap_or_default();
    let outcome = parse_callback_request_line(first_line, expected_state);
    let body = match &outcome {
        Ok(CallbackOutcome::Code(_)) => "GitHub Copilot login complete. You can return to Rho.",
        Ok(CallbackOutcome::Error(_)) | Err(_) => {
            "GitHub Copilot login failed. You can return to Rho for details."
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

async fn accept_callback(
    listeners: &CallbackListeners,
) -> Result<TcpStream, GitHubCopilotOAuthError> {
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
            return Err(GitHubCopilotOAuthError::CallbackIo(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "no OAuth callback listeners were available",
            )));
        }
    }
    .map(|(stream, _)| stream)
    .map_err(GitHubCopilotOAuthError::CallbackIo)
}

pub fn parse_callback_request_line(
    request_line: &str,
    expected_state: &str,
) -> Result<CallbackOutcome, GitHubCopilotOAuthError> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" || target.is_empty() {
        return Err(GitHubCopilotOAuthError::InvalidCallback(
            "expected GET callback request".into(),
        ));
    }
    let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|err| {
        GitHubCopilotOAuthError::InvalidCallback(format!("callback URL could not be parsed: {err}"))
    })?;
    if url.path() != CALLBACK_PATH {
        return Err(GitHubCopilotOAuthError::InvalidCallback(format!(
            "callback path was not {CALLBACK_PATH}"
        )));
    }
    let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
    let state = query.get("state").ok_or_else(|| {
        GitHubCopilotOAuthError::InvalidCallback("callback was missing state".into())
    })?;
    if state != expected_state {
        return Err(GitHubCopilotOAuthError::InvalidCallback(
            "callback state did not match".into(),
        ));
    }
    if let Some(error) = query.get("error") {
        return Ok(CallbackOutcome::Error(error.clone()));
    }
    let code = query.get("code").ok_or_else(|| {
        GitHubCopilotOAuthError::InvalidCallback("callback was missing code".into())
    })?;
    Ok(CallbackOutcome::Code(code.clone()))
}

async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
    config: &GitHubCopilotOAuthConfig,
) -> Result<GitHubCopilotTokens, GitHubCopilotOAuthError> {
    exchange_code_with_endpoint(client, code, redirect_uri, verifier, config, TOKEN_URL).await
}

async fn exchange_code_with_endpoint(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
    config: &GitHubCopilotOAuthConfig,
    endpoint: &str,
) -> Result<GitHubCopilotTokens, GitHubCopilotOAuthError> {
    let response: TokenResponse = client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .form(&access_token_form(code, redirect_uri, verifier, config))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(error) = response.error {
        return Err(GitHubCopilotOAuthError::OAuthDenied(
            response.error_description.unwrap_or(error),
        ));
    }

    let access_token = response
        .access_token
        .ok_or(GitHubCopilotOAuthError::MissingToken("access_token"))?;

    Ok(GitHubCopilotTokens {
        github_access_token: access_token,
        copilot_token: None,
        copilot_expires_at_unix: None,
        copilot_refresh_after_unix: None,
        copilot_token_endpoint: None,
        copilot_chat_endpoint: None,
        copilot_models_endpoint: None,
    })
}

fn access_token_form<'a>(
    code: &'a str,
    redirect_uri: &'a str,
    verifier: &'a str,
    config: &'a GitHubCopilotOAuthConfig,
) -> Vec<(&'static str, &'a str)> {
    vec![
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn github_copilot_browser_oauth_request_uses_pkce_and_localhost_callback() {
        let request = build_oauth_request_with_values(
            "client-id".into(),
            "state".into(),
            "verifier".into(),
            "http://localhost:1456/auth/github-copilot/callback".into(),
        );
        let url = Url::parse(&request.authorize_url).unwrap();
        let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();

        assert_eq!(url.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
        assert_eq!(query.get("response_type"), Some(&"code".to_string()));
        assert_eq!(query.get("client_id"), Some(&"client-id".to_string()));
        assert_eq!(query.get("scope"), Some(&"read:user".to_string()));
        assert_eq!(query.get("state"), Some(&"state".to_string()));
        assert_eq!(
            query.get("code_challenge_method"),
            Some(&"S256".to_string())
        );
        assert_eq!(
            query.get("code_challenge"),
            Some(&pkce_challenge("verifier"))
        );
        assert_eq!(
            request.redirect_uri,
            "http://localhost:1456/auth/github-copilot/callback"
        );
    }

    #[test]
    fn github_copilot_callback_parses_code_and_errors() {
        assert_eq!(
            parse_callback_request_line(
                "GET /auth/github-copilot/callback?code=code&state=state HTTP/1.1",
                "state",
            )
            .unwrap(),
            CallbackOutcome::Code("code".into())
        );
        assert_eq!(
            parse_callback_request_line(
                "GET /auth/github-copilot/callback?error=access_denied&state=state HTTP/1.1",
                "state",
            )
            .unwrap(),
            CallbackOutcome::Error("access_denied".into())
        );
        assert!(parse_callback_request_line(
            "GET /auth/github-copilot/callback?code=code&state=wrong HTTP/1.1",
            "state",
        )
        .is_err());
    }

    #[tokio::test]
    async fn github_copilot_token_exchange_parses_success_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let len = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.contains("POST / HTTP/1.1"));
            assert!(request.contains("client_id=client-id"));
            assert!(request.contains("client_secret=client-secret"));
            assert!(request.contains("code=code"));
            assert!(request.contains(
                "redirect_uri=http%3A%2F%2Flocalhost%3A1456%2Fauth%2Fgithub-copilot%2Fcallback"
            ));
            assert!(request.contains("code_verifier=verifier"));
            let body = r#"{"access_token":"github"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let tokens = exchange_code_with_endpoint(
            &reqwest::Client::new(),
            "code",
            "http://localhost:1456/auth/github-copilot/callback",
            "verifier",
            &GitHubCopilotOAuthConfig {
                client_id: "client-id".into(),
                client_secret: "client-secret".into(),
            },
            &endpoint,
        )
        .await
        .unwrap();

        assert_eq!(tokens.github_access_token, "github");
        assert_eq!(tokens.copilot_token, None);
    }

    #[tokio::test]
    async fn github_copilot_token_exchange_maps_oauth_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            let body = r#"{"error":"bad_verification_code","error_description":"bad code"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let err = exchange_code_with_endpoint(
            &reqwest::Client::new(),
            "code",
            "http://localhost:1456/auth/github-copilot/callback",
            "verifier",
            &GitHubCopilotOAuthConfig {
                client_id: "client-id".into(),
                client_secret: "client-secret".into(),
            },
            &endpoint,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "GitHub Copilot OAuth was denied or failed: bad code"
        );
    }
}
