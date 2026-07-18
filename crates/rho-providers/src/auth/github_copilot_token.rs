use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::StatusCode;
use serde::Deserialize;

use crate::{
    auth::github_copilot_device,
    credentials::{
        load_github_copilot_tokens, save_github_copilot_tokens, CredentialStore,
        GitHubCopilotTokens,
    },
    model::ModelError,
    provider::{self, ProviderId},
};

pub(crate) const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
pub(crate) const COPILOT_CHAT_COMPLETIONS_URL: &str =
    "https://api.githubcopilot.com/chat/completions";
pub(crate) const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const TOKEN_EXPIRY_SKEW_SECONDS: i64 = 60;
const GITHUB_TOKEN_EXPIRY_SKEW_SECONDS: i64 = 300;

#[derive(Clone)]
pub struct GitHubCopilotAuthManager {
    store: Arc<dyn CredentialStore>,
    credential: GitHubCopilotCredential,
}

#[derive(Clone)]
enum GitHubCopilotCredential {
    Env(String),
    Store(Arc<tokio::sync::Mutex<GitHubCopilotTokens>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitHubCopilotAuthSource {
    Env,
    Store,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct GitHubCopilotAuthMaterial {
    pub(crate) token: String,
    pub(crate) source: GitHubCopilotAuthSource,
    pub(crate) chat_endpoint: String,
    pub(crate) models_endpoint: String,
}

impl std::fmt::Debug for GitHubCopilotAuthMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubCopilotAuthMaterial")
            .field("token", &"[REDACTED]")
            .field("source", &self.source)
            .field("chat_endpoint", &self.chat_endpoint)
            .field("models_endpoint", &self.models_endpoint)
            .finish()
    }
}

#[derive(Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: Option<i64>,
    refresh_in: Option<i64>,
    endpoints: Option<CopilotTokenEndpoints>,
}

#[derive(Deserialize)]
struct CopilotTokenEndpoints {
    api: Option<String>,
    #[serde(alias = "chat_completions")]
    chat: Option<String>,
    models: Option<String>,
}

impl GitHubCopilotAuthManager {
    pub(crate) fn new(store: Arc<dyn CredentialStore>) -> Result<Self, ModelError> {
        Self::from_acquired(store, nonempty_env_copilot_token())
    }

    fn from_acquired(
        store: Arc<dyn CredentialStore>,
        env_token: Option<String>,
    ) -> Result<Self, ModelError> {
        let credential = match nonempty_token(env_token) {
            Some(token) => GitHubCopilotCredential::Env(token),
            None => GitHubCopilotCredential::Store(Arc::new(tokio::sync::Mutex::new(
                load_github_copilot_tokens(store.as_ref())?
                    .ok_or(ModelError::MissingGithubCopilotAuth)?,
            ))),
        };
        Ok(Self { store, credential })
    }

    #[cfg(test)]
    pub(crate) fn new_with_env_token(
        store: Arc<dyn CredentialStore>,
        env_token: Option<String>,
    ) -> Result<Self, ModelError> {
        Self::from_acquired(store, env_token)
    }

    pub(crate) fn ensure_auth_available(&self) -> Result<(), ModelError> {
        Ok(())
    }

    pub(crate) async fn auth_material(
        &self,
        client: &reqwest::Client,
    ) -> Result<GitHubCopilotAuthMaterial, ModelError> {
        match &self.credential {
            GitHubCopilotCredential::Env(token) => Ok(GitHubCopilotAuthMaterial {
                token: token.clone(),
                source: GitHubCopilotAuthSource::Env,
                chat_endpoint: COPILOT_CHAT_COMPLETIONS_URL.to_string(),
                models_endpoint: COPILOT_MODELS_URL.to_string(),
            }),
            GitHubCopilotCredential::Store(tokens) => {
                let mut tokens = tokens.lock().await;
                if let Some(token) = fresh_cached_copilot_token(&tokens, now_unix_seconds()) {
                    return Ok(material_from_stored_token(token, &tokens));
                }
                refresh_copilot_token_with_store(client, self.store.as_ref(), &mut tokens).await
            }
        }
    }

    pub(crate) async fn force_refresh(
        &self,
        client: &reqwest::Client,
    ) -> Result<Option<GitHubCopilotAuthMaterial>, ModelError> {
        match &self.credential {
            GitHubCopilotCredential::Env(_) => Ok(None),
            GitHubCopilotCredential::Store(tokens) => {
                let mut tokens = tokens.lock().await;
                refresh_copilot_token_with_store(client, self.store.as_ref(), &mut tokens)
                    .await
                    .map(Some)
            }
        }
    }
}

pub(crate) async fn auth_material_with_store(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
) -> Result<GitHubCopilotAuthMaterial, ModelError> {
    auth_material_with_env_token(client, store, nonempty_env_copilot_token()).await
}

async fn auth_material_with_env_token(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    env_token: Option<String>,
) -> Result<GitHubCopilotAuthMaterial, ModelError> {
    if let Some(token) = nonempty_token(env_token) {
        return Ok(GitHubCopilotAuthMaterial {
            token,
            source: GitHubCopilotAuthSource::Env,
            chat_endpoint: COPILOT_CHAT_COMPLETIONS_URL.to_string(),
            models_endpoint: COPILOT_MODELS_URL.to_string(),
        });
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
    force_refresh_auth_material_with_env_token(client, store, nonempty_env_copilot_token()).await
}

async fn force_refresh_auth_material_with_env_token(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    env_token: Option<String>,
) -> Result<Option<GitHubCopilotAuthMaterial>, ModelError> {
    if nonempty_token(env_token).is_some() {
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
    refresh_github_token_if_needed(client, store, tokens).await?;
    let endpoint = tokens
        .copilot_token_endpoint
        .clone()
        .unwrap_or_else(|| COPILOT_TOKEN_URL.to_string());
    let mut response =
        request_copilot_token(client, &endpoint, &tokens.github_access_token).await?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) && refresh_github_token(client, store, tokens).await?
    {
        response = request_copilot_token(client, &endpoint, &tokens.github_access_token).await?;
    }
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
        apply_token_endpoints(tokens, endpoints);
    }
    save_github_copilot_tokens(store, tokens)?;
    Ok(material_from_stored_token(response.token, tokens))
}

async fn request_copilot_token(
    client: &reqwest::Client,
    endpoint: &str,
    github_access_token: &str,
) -> Result<reqwest::Response, ModelError> {
    Ok(client
        .get(endpoint)
        .header("Authorization", format!("token {github_access_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", crate::rho_user_agent())
        .send()
        .await?)
}

async fn refresh_github_token_if_needed(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut GitHubCopilotTokens,
) -> Result<(), ModelError> {
    if tokens.github_expires_at_unix.is_some_and(|expires_at| {
        expires_at <= now_unix_seconds() + GITHUB_TOKEN_EXPIRY_SKEW_SECONDS
    }) {
        let _ = refresh_github_token(client, store, tokens).await?;
    }
    Ok(())
}

async fn refresh_github_token(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut GitHubCopilotTokens,
) -> Result<bool, ModelError> {
    let Some(refresh_token) = tokens.github_refresh_token.as_deref() else {
        return Ok(false);
    };
    let refreshed =
        github_copilot_device::refresh_github_copilot_github_token(client, refresh_token)
            .await
            .map_err(|_| ModelError::MissingGithubCopilotAuth)?;
    tokens.github_access_token = refreshed.access_token;
    if refreshed.refresh_token.is_some() {
        tokens.github_refresh_token = refreshed.refresh_token;
    }
    tokens.github_expires_at_unix = refreshed.expires_at_unix;
    save_github_copilot_tokens(store, tokens)?;
    Ok(true)
}

fn apply_token_endpoints(tokens: &mut GitHubCopilotTokens, endpoints: CopilotTokenEndpoints) {
    let api_chat_endpoint = endpoints
        .api
        .as_deref()
        .map(|api| append_endpoint_path(api, "chat/completions"));
    let api_models_endpoint = endpoints
        .api
        .as_deref()
        .map(|api| append_endpoint_path(api, "models"));
    tokens.copilot_chat_endpoint = endpoints.chat.or(api_chat_endpoint);
    tokens.copilot_models_endpoint = endpoints.models.or(api_models_endpoint);
}

fn append_endpoint_path(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
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

fn nonempty_env_copilot_token() -> Option<String> {
    let env_var = provider::provider_descriptor_by_id(ProviderId::GithubCopilot)
        .auth_kind
        .env_var();
    nonempty_token(std::env::var(env_var).ok())
}

fn nonempty_token(token: Option<String>) -> Option<String> {
    token.filter(|token| !token.trim().is_empty())
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
            github_refresh_token: None,
            github_expires_at_unix: None,
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
            "{{\"token\":\"copilot\",\"expires_at\":2000,\"refresh_in\":120,\"endpoints\":{{\"api\":\"{url}\"}}}}"
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
        tokens.copilot_token_endpoint = Some(url.clone());

        let material =
            refresh_copilot_token_with_store(&reqwest::Client::new(), &store, &mut tokens)
                .await
                .unwrap();

        assert_eq!(material.token, "copilot");
        assert_eq!(material.chat_endpoint, format!("{url}/chat/completions"));
        assert_eq!(material.models_endpoint, format!("{url}/models"));
        let stored = load_github_copilot_tokens(&store).unwrap().unwrap();
        assert_eq!(stored.copilot_token.as_deref(), Some("copilot"));
        let expected_chat_endpoint = format!("{url}/chat/completions");
        assert_eq!(
            stored.copilot_chat_endpoint.as_deref(),
            Some(expected_chat_endpoint.as_str())
        );
    }

    #[test]
    fn token_endpoints_build_full_paths_from_api_base() {
        let mut tokens = tokens(None, None, None);

        apply_token_endpoints(
            &mut tokens,
            CopilotTokenEndpoints {
                api: Some("https://copilot.example/api/".into()),
                chat: None,
                models: None,
            },
        );

        assert_eq!(
            tokens.copilot_chat_endpoint.as_deref(),
            Some("https://copilot.example/api/chat/completions")
        );
        assert_eq!(
            tokens.copilot_models_endpoint.as_deref(),
            Some("https://copilot.example/api/models")
        );
    }

    #[tokio::test]
    async fn empty_env_token_does_not_disable_stored_refresh() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            let body = r#"{"token":"refreshed"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let store = Arc::new(MemoryCredentialStore::default());
        let mut tokens = tokens(Some("stale"), Some(2_000), None);
        tokens.copilot_token_endpoint = Some(url);
        save_github_copilot_tokens(store.as_ref(), &tokens).unwrap();

        let auth =
            GitHubCopilotAuthManager::new_with_env_token(store, Some(String::new())).unwrap();
        let material = auth
            .force_refresh(&reqwest::Client::new())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(material.token, "refreshed");
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

    #[test]
    fn auth_material_debug_redacts_token() {
        let material = GitHubCopilotAuthMaterial {
            token: "copilot-secret-token".into(),
            source: GitHubCopilotAuthSource::Env,
            chat_endpoint: "https://chat.example".into(),
            models_endpoint: "https://models.example".into(),
        };

        let debug = format!("{material:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("copilot-secret-token"));
    }
}
