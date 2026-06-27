use std::time::Duration;

use serde::Deserialize;

use crate::credentials::GitHubCopilotTokens;

pub const GITHUB_COPILOT_CLIENT_ID_ENV: &str = "RHO_GITHUB_COPILOT_CLIENT_ID";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEFAULT_SCOPE: &str = "read:user";
const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(900);
const USER_AGENT: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubCopilotDeviceFlow {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    client_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubCopilotOAuthError {
    #[error("GitHub Copilot login requires an app-owned GitHub OAuth client id; set {GITHUB_COPILOT_CLIENT_ID_ENV} and retry /login github-copilot")]
    MissingClientId,
    #[error("GitHub device flow request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("GitHub device flow response was missing {0}")]
    MissingField(&'static str),
    #[error("GitHub Copilot login timed out before authorization completed")]
    Timeout,
    #[error("GitHub Copilot login was denied by the user")]
    AccessDenied,
    #[error("GitHub Copilot device code expired; run /login github-copilot again")]
    ExpiredToken,
    #[error("GitHub device flow failed: {0}")]
    OAuth(String),
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenPollResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn start_github_copilot_device_flow(
) -> Result<GitHubCopilotDeviceFlow, GitHubCopilotOAuthError> {
    let client_id = std::env::var(GITHUB_COPILOT_CLIENT_ID_ENV)
        .map_err(|_| GitHubCopilotOAuthError::MissingClientId)?;
    start_github_copilot_device_flow_with_client(&reqwest::Client::new(), client_id).await
}

async fn start_github_copilot_device_flow_with_client(
    client: &reqwest::Client,
    client_id: String,
) -> Result<GitHubCopilotDeviceFlow, GitHubCopilotOAuthError> {
    start_github_copilot_device_flow_with_endpoint(client, client_id, DEVICE_CODE_URL).await
}

async fn start_github_copilot_device_flow_with_endpoint(
    client: &reqwest::Client,
    client_id: String,
    endpoint: &str,
) -> Result<GitHubCopilotDeviceFlow, GitHubCopilotOAuthError> {
    let response: DeviceCodeResponse = client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .form(&device_code_form(&client_id))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(GitHubCopilotDeviceFlow {
        device_code: response
            .device_code
            .ok_or(GitHubCopilotOAuthError::MissingField("device_code"))?,
        user_code: response
            .user_code
            .ok_or(GitHubCopilotOAuthError::MissingField("user_code"))?,
        verification_uri: response
            .verification_uri
            .ok_or(GitHubCopilotOAuthError::MissingField("verification_uri"))?,
        expires_in: response.expires_in.unwrap_or(900),
        interval: response.interval.unwrap_or(5).max(1),
        client_id,
    })
}

pub async fn poll_github_copilot_device_flow(
    flow: GitHubCopilotDeviceFlow,
) -> Result<GitHubCopilotTokens, GitHubCopilotOAuthError> {
    poll_github_copilot_device_flow_with_client(&reqwest::Client::new(), flow, DEFAULT_POLL_TIMEOUT)
        .await
}

async fn poll_github_copilot_device_flow_with_client(
    client: &reqwest::Client,
    flow: GitHubCopilotDeviceFlow,
    timeout: Duration,
) -> Result<GitHubCopilotTokens, GitHubCopilotOAuthError> {
    poll_github_copilot_device_flow_with_endpoint(client, flow, timeout, ACCESS_TOKEN_URL).await
}

async fn poll_github_copilot_device_flow_with_endpoint(
    client: &reqwest::Client,
    flow: GitHubCopilotDeviceFlow,
    timeout: Duration,
    endpoint: &str,
) -> Result<GitHubCopilotTokens, GitHubCopilotOAuthError> {
    let deadline = tokio::time::Instant::now() + timeout.min(Duration::from_secs(flow.expires_in));
    let mut interval = Duration::from_secs(flow.interval);
    loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(GitHubCopilotOAuthError::Timeout);
        }

        let response: TokenPollResponse = client
            .post(endpoint)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .form(&access_token_form(&flow.client_id, &flow.device_code))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if let Some(access_token) = response.access_token {
            return Ok(GitHubCopilotTokens {
                github_access_token: access_token,
                copilot_token: None,
                copilot_expires_at_unix: None,
                copilot_refresh_after_unix: None,
                copilot_token_endpoint: None,
                copilot_chat_endpoint: None,
                copilot_models_endpoint: None,
            });
        }

        match response.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => return Err(GitHubCopilotOAuthError::ExpiredToken),
            Some("access_denied") => return Err(GitHubCopilotOAuthError::AccessDenied),
            Some(error) => {
                return Err(GitHubCopilotOAuthError::OAuth(
                    response
                        .error_description
                        .unwrap_or_else(|| error.to_string()),
                ))
            }
            None => return Err(GitHubCopilotOAuthError::MissingField("access_token")),
        }
    }
}

fn device_code_form(client_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("client_id", client_id.to_string()),
        ("scope", DEFAULT_SCOPE.to_string()),
    ]
}

fn access_token_form(client_id: &str, device_code: &str) -> Vec<(&'static str, String)> {
    vec![
        ("client_id", client_id.to_string()),
        ("device_code", device_code.to_string()),
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
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
    fn device_code_form_uses_app_client_and_read_user_scope() {
        assert_eq!(
            device_code_form("client-id"),
            vec![
                ("client_id", "client-id".to_string()),
                ("scope", "read:user".to_string()),
            ]
        );
    }

    #[test]
    fn access_token_form_uses_device_flow_grant() {
        assert_eq!(
            access_token_form("client-id", "device-code"),
            vec![
                ("client_id", "client-id".to_string()),
                ("device_code", "device-code".to_string()),
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn device_flow_start_parses_success_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            let body = r#"{"device_code":"device","user_code":"user","verification_uri":"https://github.com/login/device","expires_in":600,"interval":0}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let flow = start_github_copilot_device_flow_with_endpoint(
            &reqwest::Client::new(),
            "client".into(),
            &endpoint,
        )
        .await
        .unwrap();

        assert_eq!(flow.device_code, "device");
        assert_eq!(flow.user_code, "user");
        assert_eq!(flow.interval, 1);
    }

    #[tokio::test]
    async fn device_flow_poll_handles_pending_slowdown_then_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            for body in [
                r#"{"error":"authorization_pending"}"#,
                r#"{"error":"slow_down"}"#,
                r#"{"access_token":"github"}"#,
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 1024];
                let _ = stream.read(&mut buffer).await.unwrap();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(), body
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let flow = GitHubCopilotDeviceFlow {
            device_code: "device".into(),
            user_code: "user".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 900,
            interval: 0,
            client_id: "client".into(),
        };

        let tokens = poll_github_copilot_device_flow_with_endpoint(
            &reqwest::Client::new(),
            flow,
            Duration::from_secs(30),
            &endpoint,
        )
        .await
        .unwrap();

        assert_eq!(tokens.github_access_token, "github");
    }

    #[tokio::test]
    async fn device_flow_poll_maps_expired_and_denied_errors() {
        for (body, expected) in [
            (
                r#"{"error":"expired_token"}"#,
                GitHubCopilotOAuthError::ExpiredToken,
            ),
            (
                r#"{"error":"access_denied"}"#,
                GitHubCopilotOAuthError::AccessDenied,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 1024];
                let _ = stream.read(&mut buffer).await.unwrap();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(), body
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            });
            let flow = GitHubCopilotDeviceFlow {
                device_code: "device".into(),
                user_code: "user".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_in: 900,
                interval: 0,
                client_id: "client".into(),
            };

            let err = poll_github_copilot_device_flow_with_endpoint(
                &reqwest::Client::new(),
                flow,
                Duration::from_secs(30),
                &endpoint,
            )
            .await
            .unwrap_err();

            assert_eq!(err.to_string(), expected.to_string());
        }
    }
}
