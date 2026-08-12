use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;

use crate::{
    auth::cursor_oauth::{refresh_cursor_tokens, CursorOAuthError},
    credentials::{load_cursor_tokens, save_cursor_tokens, CredentialStore, CursorTokens},
    model::ModelError,
    provider,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorAuthSource {
    Env,
    Store,
}

pub struct CursorAuthManager {
    client: reqwest::Client,
    store: Arc<dyn CredentialStore>,
    source: CursorAuthSource,
    tokens: Mutex<CursorTokens>,
}

impl CursorAuthManager {
    pub(crate) fn from_tokens(
        store: Arc<dyn CredentialStore>,
        source: CursorAuthSource,
        tokens: CursorTokens,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            store,
            source,
            tokens: Mutex::new(tokens),
        }
    }

    pub(crate) async fn access_token(&self) -> Result<String, ModelError> {
        let mut tokens = self.tokens.lock().await;
        if self.source == CursorAuthSource::Store && token_is_expiring(&tokens) {
            refresh_locked(&self.client, self.store.as_ref(), &mut tokens).await?;
        }
        Ok(tokens.access_token.clone())
    }

    pub(crate) async fn force_refresh(
        &self,
        rejected_token: &str,
    ) -> Result<Option<String>, ModelError> {
        if self.source == CursorAuthSource::Env {
            return Ok(None);
        }
        let mut tokens = self.tokens.lock().await;
        if tokens.access_token != rejected_token {
            return Ok(Some(tokens.access_token.clone()));
        }
        refresh_locked(&self.client, self.store.as_ref(), &mut tokens).await?;
        Ok(Some(tokens.access_token.clone()))
    }
}

/// One-shot env-or-store token for discovery. Live turns use [`CursorAuthManager`].
pub(crate) async fn resolve_cursor_access_token(
    store: &dyn CredentialStore,
    client: &reqwest::Client,
) -> Result<String, ModelError> {
    let env_var = provider::provider_descriptor("cursor")
        .and_then(|descriptor| descriptor.default_auth().auth_kind.env_var())
        .expect("Cursor OAuth must declare an environment variable");
    if let Ok(token) = std::env::var(env_var) {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    let missing = || crate::model::registry::missing_credentials_error("cursor");
    let mut tokens = load_cursor_tokens(store)?.ok_or_else(missing)?;
    if token_is_expiring(&tokens) {
        refresh_locked(client, store, &mut tokens).await?;
    }
    Ok(tokens.access_token)
}

async fn refresh_locked(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut CursorTokens,
) -> Result<(), ModelError> {
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .ok_or(crate::model::registry::missing_credentials_error("cursor"))?;
    let refreshed =
        refresh_cursor_tokens(client, refresh_token)
            .await
            .map_err(|error| match error {
                CursorOAuthError::Unauthorized(_) => {
                    crate::model::registry::missing_credentials_error("cursor")
                }
                error => ModelError::InvalidResponse(error.to_string()),
            })?;
    save_cursor_tokens(store, &refreshed)?;
    *tokens = refreshed;
    Ok(())
}

pub(crate) fn token_is_expiring(tokens: &CursorTokens) -> bool {
    tokens
        .expires_at_unix
        .is_some_and(|expires| expires <= now_unix())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
#[path = "cursor_token_tests.rs"]
mod tests;
