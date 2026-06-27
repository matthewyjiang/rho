use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::StatusCode;
use serde::Deserialize;

use crate::{
    credentials::{
        load_github_copilot_tokens, save_github_copilot_tokens, CredentialStore,
        GitHubCopilotTokens,
    },
    model::ModelError,
};

pub(crate) const GITHUB_COPILOT_TOKEN_ENV: &str = "GITHUB_COPILOT_TOKEN";
pub(crate) const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
pub(crate) const COPILOT_CHAT_COMPLETIONS_URL: &str =
    "https://api.githubcopilot.com/chat/completions";
pub(crate) const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const USER_AGENT: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));
const TOKEN_EXPIRY_SKEW_SECONDS: i64 = 60;

#[derive(Clone)]
pub(crate) struct GitHubCopilotAuthManager {
    store: Arc<dyn CredentialStore>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitHubCopilotAuthSource {
    Env,
    Store,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitHubCopilotAuthMaterial {
    pub(crate) token: String,
    pub(crate) source: GitHubCopilotAuthSource,
    pub(crate) chat_endpoint: String,
    pub(crate) models_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: Option<i64>,
    refresh_in: Option<i64>,
    endpoints: Option<CopilotTokenEndpoints>,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenEndpoints {
    api: Option<String>,
    #[serde(alias = "chat_completions")]
    chat: Option<String>,
    models: Option<String>,
}

impl GitHubCopilotAuthManager {
    pub(crate) fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self { store }
    }

    pub(crate) async fn auth_material(
        &self,
        client: &reqwest::Client,
    ) -> Result<GitHubCopilotAuthMaterial, ModelError> {
        auth_material_with_store(client, self.store.as_ref()).await
    }

    pub(crate) async fn force_refresh(
        &self,
        client: &reqwest::Client,
    ) -> Result<Option<GitHubCopilotAuthMaterial>, ModelError> {
        force_refresh_auth_material_with_store(client, self.store.as_ref()).await
    }
}

pub(crate) async fn auth_material_with_store(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
) -> Result<GitHubCopilotAuthMaterial, ModelError> {
    if let Ok(token) = std::env::var(GITHUB_COPILOT_TOKEN_ENV) {
        if !token.trim().is_empty() {
            return Ok(GitHubCopilotAuthMaterial {
                token,
                source: GitHubCopilotAuthSource::Env,
                chat_endpoint: COPILOT_CHAT_COMPLETIONS_URL.to_string(),
                models_endpoint: COPILOT_MODELS_URL.to_string(),
            });
        }
    }

    let mut tokens =
        load_github_copilot_tokens(store)?.ok_or(ModelError::MissingGithubCopilotAuth)?;
    if let Some(token) = fresh_cached_copilot_token(&tokens, now_unix_seconds()) {
        return Ok(material_from_stored_token(token, &tokens));
    }

    refresh_copilot_token_with_store(client, store, &mut tokens).await
}

pub(crate) async fn force_refresh_auth_material_with_store(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
) -> Result<Option<GitHubCopilotAuthMaterial>, ModelError> {
    if std::env::var_os(GITHUB_COPILOT_TOKEN_ENV).is_some() {
        return Ok(None);
    }
    let Some(mut tokens) = load_github_copilot_tokens(store)? else {
        return Ok(None);
    };
    refresh_copilot_token_with_store(client, store, &mut tokens)
        .await
        .map(Some)
}

async fn refresh_copilot_token_with_store(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut GitHubCopilotTokens,
) -> Result<GitHubCopilotAuthMaterial, ModelError> {
    let endpoint = tokens
        .copilot_token_endpoint
        .as_deref()
        .unwrap_or(COPILOT_TOKEN_URL);
    let response = client
        .get(endpoint)
        .header(
            "Authorization",
            format!("token {}", tokens.github_access_token),
        )
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .send()
        .await?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(ModelError::MissingGithubCopilotAuth);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ModelError::HttpStatus { status, body });
    }
    let response: CopilotTokenResponse = response.json().await?;

    let now = now_unix_seconds();
    tokens.copilot_token = Some(response.token.clone());
    tokens.copilot_expires_at_unix = response.expires_at;
    tokens.copilot_refresh_after_unix = response.refresh_in.map(|seconds| now + seconds);
    if let Some(endpoints) = response.endpoints {
        tokens.copilot_chat_endpoint = endpoints.chat.or(endpoints.api);
        tokens.copilot_models_endpoint = endpoints.models;
    }
    save_github_copilot_tokens(store, tokens)?;
    Ok(material_from_stored_token(response.token, tokens))
}

fn material_from_stored_token(
    token: String,
    tokens: &GitHubCopilotTokens,
) -> GitHubCopilotAuthMaterial {
    GitHubCopilotAuthMaterial {
        token,
        source: GitHubCopilotAuthSource::Store,
        chat_endpoint: tokens
            .copilot_chat_endpoint
            .clone()
            .unwrap_or_else(|| COPILOT_CHAT_COMPLETIONS_URL.to_string()),
        models_endpoint: tokens
            .copilot_models_endpoint
            .clone()
            .unwrap_or_else(|| COPILOT_MODELS_URL.to_string()),
    }
}

pub(crate) fn fresh_cached_copilot_token(
    tokens: &GitHubCopilotTokens,
    now_unix: i64,
) -> Option<String> {
    let token = tokens.copilot_token.as_ref()?.trim();
    if token.is_empty() {
        return None;
    }
    if tokens
        .copilot_expires_at_unix
        .is_some_and(|expires_at| expires_at <= now_unix + TOKEN_EXPIRY_SKEW_SECONDS)
    {
        return None;
    }
    if tokens
        .copilot_refresh_after_unix
        .is_some_and(|refresh_after| refresh_after <= now_unix)
    {
        return None;
    }
    Some(token.to_string())
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
    use crate::credentials::MemoryCredentialStore;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn tokens(
        copilot_token: Option<&str>,
        expires_at: Option<i64>,
        refresh_after: Option<i64>,
    ) -> GitHubCopilotTokens {
        GitHubCopilotTokens {
            github_access_token: "github".into(),
            copilot_token: copilot_token.map(str::to_string),
            copilot_expires_at_unix: expires_at,
            copilot_refresh_after_unix: refresh_after,
            copilot_token_endpoint: None,
            copilot_chat_endpoint: None,
            copilot_models_endpoint: None,
        }
    }

    #[test]
    fn cached_copilot_token_is_fresh_before_refresh_and_expiry() {
        assert_eq!(
            fresh_cached_copilot_token(&tokens(Some("copilot"), Some(1_000), Some(900)), 800),
            Some("copilot".into())
        );
    }

    #[test]
    fn cached_copilot_token_refreshes_near_expiry_or_after_refresh_time() {
        assert_eq!(
            fresh_cached_copilot_token(&tokens(Some("copilot"), Some(850), None), 800),
            None
        );
        assert_eq!(
            fresh_cached_copilot_token(&tokens(Some("copilot"), Some(1_000), Some(799)), 800),
            None
        );
    }

    #[test]
    fn cached_material_uses_stored_endpoints() {
        let mut tokens = tokens(Some("copilot"), Some(1_000), None);
        tokens.copilot_chat_endpoint = Some("http://chat.test".into());
        tokens.copilot_models_endpoint = Some("http://models.test".into());

        let material = material_from_stored_token("copilot".into(), &tokens);

        assert_eq!(material.chat_endpoint, "http://chat.test");
        assert_eq!(material.models_endpoint, "http://models.test");
    }

    #[tokio::test]
    async fn token_exchange_persists_endpoints_and_returns_material() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "{{\"token\":\"copilot\",\"expires_at\":2000,\"refresh_in\":120,\"endpoints\":{{\"chat\":\"{url}/chat\",\"models\":\"{url}/models\"}}}}"
        );
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                response.len(),
                response
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let store = MemoryCredentialStore::default();
        let mut tokens = tokens(None, None, None);
        tokens.copilot_token_endpoint = Some(url);

        let material =
            refresh_copilot_token_with_store(&reqwest::Client::new(), &store, &mut tokens)
                .await
                .unwrap();

        assert_eq!(material.token, "copilot");
        assert!(material.chat_endpoint.ends_with("/chat"));
        assert!(material.models_endpoint.ends_with("/models"));
        let stored = load_github_copilot_tokens(&store).unwrap().unwrap();
        assert_eq!(stored.copilot_token.as_deref(), Some("copilot"));
        assert!(stored.copilot_chat_endpoint.unwrap().ends_with("/chat"));
    }

    #[tokio::test]
    async fn token_exchange_maps_unauthorized_to_missing_auth() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let store = MemoryCredentialStore::default();
        let mut tokens = tokens(None, None, None);
        tokens.copilot_token_endpoint = Some(url);

        let err = refresh_copilot_token_with_store(&reqwest::Client::new(), &store, &mut tokens)
            .await
            .unwrap_err();

        assert!(matches!(err, ModelError::MissingGithubCopilotAuth));
    }
}
