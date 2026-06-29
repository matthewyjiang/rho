use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio::time::sleep;

use crate::credentials::GitHubCopilotTokens;

const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEFAULT_SCOPE: &str = "read:user";
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct GitHubCopilotDeviceLogin {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: Duration,
    device_code: String,
    interval: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubCopilotDeviceError {
    #[error("GitHub Copilot device login setup failed: {0}")]
    Setup(String),
    #[error("GitHub Copilot device login failed: {0}")]
    OAuthDenied(String),
    #[error("GitHub Copilot device login request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("GitHub Copilot device login response was missing {0}")]
    MissingField(&'static str),
    #[error("timed out waiting for GitHub Copilot device login")]
    Timeout,
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

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitHubCopilotGithubToken {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_at_unix: Option<i64>,
}

pub async fn start_github_copilot_device_login(
) -> Result<GitHubCopilotDeviceLogin, GitHubCopilotDeviceError> {
    start_github_copilot_device_login_with_endpoint(&http_client()?, DEVICE_CODE_URL).await
}

fn http_client() -> Result<reqwest::Client, GitHubCopilotDeviceError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(GitHubCopilotDeviceError::Request)
}

async fn start_github_copilot_device_login_with_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
) -> Result<GitHubCopilotDeviceLogin, GitHubCopilotDeviceError> {
    let response: DeviceCodeResponse = client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .form(&[("client_id", CLIENT_ID), ("scope", DEFAULT_SCOPE)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(error) = response.error {
        return Err(GitHubCopilotDeviceError::Setup(
            response.error_description.unwrap_or(error),
        ));
    }

    Ok(GitHubCopilotDeviceLogin {
        device_code: required(response.device_code, "device_code")?,
        user_code: required(response.user_code, "user_code")?,
        verification_uri: required(response.verification_uri, "verification_uri")?,
        verification_uri_complete: response.verification_uri_complete,
        expires_in: Duration::from_secs(required(response.expires_in, "expires_in")?),
        interval: response
            .interval
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_POLL_INTERVAL),
    })
}

fn required<T>(value: Option<T>, field: &'static str) -> Result<T, GitHubCopilotDeviceError> {
    value.ok_or(GitHubCopilotDeviceError::MissingField(field))
}

pub async fn complete_github_copilot_device_login(
    login: GitHubCopilotDeviceLogin,
) -> Result<GitHubCopilotTokens, GitHubCopilotDeviceError> {
    complete_github_copilot_device_login_with_endpoint(&http_client()?, login, TOKEN_URL).await
}

async fn complete_github_copilot_device_login_with_endpoint(
    client: &reqwest::Client,
    login: GitHubCopilotDeviceLogin,
    endpoint: &str,
) -> Result<GitHubCopilotTokens, GitHubCopilotDeviceError> {
    let deadline = Instant::now() + login.expires_in;
    let mut interval = login.interval;

    loop {
        if Instant::now() >= deadline {
            return Err(GitHubCopilotDeviceError::Timeout);
        }
        sleep(interval).await;

        let response: TokenResponse = client
            .post(endpoint)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", login.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if let Some(access_token) = response.access_token {
            return Ok(GitHubCopilotTokens {
                github_access_token: access_token,
                github_refresh_token: response.refresh_token,
                github_expires_at_unix: response
                    .expires_in
                    .map(|seconds| now_unix_seconds() + seconds),
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
            Some("slow_down") => interval += SLOW_DOWN_INCREMENT,
            Some("expired_token") => return Err(GitHubCopilotDeviceError::Timeout),
            Some("access_denied") => {
                return Err(GitHubCopilotDeviceError::OAuthDenied(
                    response
                        .error_description
                        .unwrap_or_else(|| "access denied".into()),
                ));
            }
            Some(error) => {
                return Err(GitHubCopilotDeviceError::OAuthDenied(
                    response.error_description.unwrap_or_else(|| error.into()),
                ));
            }
            None => return Err(GitHubCopilotDeviceError::MissingField("access_token")),
        }
    }
}

pub(crate) async fn refresh_github_copilot_github_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<GitHubCopilotGithubToken, GitHubCopilotDeviceError> {
    refresh_github_copilot_github_token_with_endpoint(client, refresh_token, TOKEN_URL).await
}

async fn refresh_github_copilot_github_token_with_endpoint(
    client: &reqwest::Client,
    refresh_token: &str,
    endpoint: &str,
) -> Result<GitHubCopilotGithubToken, GitHubCopilotDeviceError> {
    let response: TokenResponse = client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .form(&[
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(error) = response.error {
        return Err(GitHubCopilotDeviceError::OAuthDenied(
            response.error_description.unwrap_or(error),
        ));
    }

    Ok(GitHubCopilotGithubToken {
        access_token: required(response.access_token, "access_token")?,
        refresh_token: response.refresh_token,
        expires_at_unix: response
            .expires_in
            .map(|seconds| now_unix_seconds() + seconds),
    })
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn github_copilot_device_login_posts_client_id_and_parses_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let len = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.contains("POST / HTTP/1.1"));
            assert!(request.contains("client_id=Iv1.b507a08c87ecfe98"));
            assert!(request.contains("scope=read%3Auser"));
            let body = r#"{"device_code":"device","user_code":"ABCD-EFGH","verification_uri":"https://github.com/login/device","verification_uri_complete":"https://github.com/login/device?user_code=ABCD-EFGH","expires_in":900,"interval":1}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let login =
            start_github_copilot_device_login_with_endpoint(&reqwest::Client::new(), &endpoint)
                .await
                .unwrap();

        assert_eq!(login.user_code, "ABCD-EFGH");
        assert_eq!(login.verification_uri, "https://github.com/login/device");
        assert_eq!(
            login.verification_uri_complete.as_deref(),
            Some("https://github.com/login/device?user_code=ABCD-EFGH")
        );
        assert_eq!(login.expires_in, Duration::from_secs(900));
        assert_eq!(login.interval, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn github_copilot_device_login_polls_until_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            for body in [
                r#"{"error":"authorization_pending"}"#,
                r#"{"access_token":"github","refresh_token":"refresh","expires_in":3600}"#,
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 2048];
                let len = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..len]);
                assert!(request.contains("client_id=Iv1.b507a08c87ecfe98"));
                assert!(request.contains("device_code=device"));
                assert!(request
                    .contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });

        let tokens = complete_github_copilot_device_login_with_endpoint(
            &reqwest::Client::new(),
            GitHubCopilotDeviceLogin {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://github.com/login/device".into(),
                verification_uri_complete: None,
                expires_in: Duration::from_secs(10),
                device_code: "device".into(),
                interval: Duration::from_millis(1),
            },
            &endpoint,
        )
        .await
        .unwrap();

        assert_eq!(tokens.github_access_token, "github");
        assert_eq!(tokens.github_refresh_token.as_deref(), Some("refresh"));
        assert!(tokens.github_expires_at_unix.is_some());
        assert_eq!(tokens.copilot_token, None);
    }

    #[tokio::test]
    async fn github_copilot_device_login_maps_access_denied() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            let body = r#"{"error":"access_denied","error_description":"denied"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });

        let err = complete_github_copilot_device_login_with_endpoint(
            &reqwest::Client::new(),
            GitHubCopilotDeviceLogin {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "https://github.com/login/device".into(),
                verification_uri_complete: None,
                expires_in: Duration::from_secs(10),
                device_code: "device".into(),
                interval: Duration::from_millis(1),
            },
            &endpoint,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "GitHub Copilot device login failed: denied"
        );
    }
}
