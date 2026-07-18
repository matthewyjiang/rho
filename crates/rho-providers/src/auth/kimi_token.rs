use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;

use crate::{
    auth::kimi_oauth::{refresh_kimi_tokens, KimiOAuthError},
    credentials::{save_kimi_tokens, CredentialStore, KimiTokens},
    model::ModelError,
};

const MIN_REFRESH_THRESHOLD_SECONDS: i64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KimiAuthSource {
    Env,
    Store,
}

pub struct KimiAuthManager {
    client: reqwest::Client,
    store: Arc<dyn CredentialStore>,
    source: KimiAuthSource,
    tokens: Mutex<KimiTokens>,
}

impl KimiAuthManager {
    pub(crate) fn from_tokens(
        store: Arc<dyn CredentialStore>,
        source: KimiAuthSource,
        tokens: KimiTokens,
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
        if self.source == KimiAuthSource::Store && token_is_expiring(&tokens) {
            refresh_locked(&self.client, self.store.as_ref(), &mut tokens).await?;
        }
        Ok(tokens.access_token.clone())
    }

    pub(crate) async fn force_refresh(
        &self,
        rejected_token: &str,
    ) -> Result<Option<String>, ModelError> {
        if self.source == KimiAuthSource::Env {
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

async fn refresh_locked(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut KimiTokens,
) -> Result<(), ModelError> {
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .ok_or(ModelError::MissingKimiAuth)?;
    let refreshed =
        refresh_kimi_tokens(client, refresh_token)
            .await
            .map_err(|error| match error {
                KimiOAuthError::Unauthorized(_) => ModelError::MissingKimiAuth,
                error => ModelError::InvalidResponse(error.to_string()),
            })?;
    save_kimi_tokens(store, &refreshed)?;
    *tokens = refreshed;
    Ok(())
}

pub(crate) fn token_is_expiring(tokens: &KimiTokens) -> bool {
    let threshold = tokens
        .expires_in
        .and_then(|seconds| i64::try_from(seconds / 2).ok())
        .unwrap_or_default()
        .max(MIN_REFRESH_THRESHOLD_SECONDS);
    tokens
        .expires_at_unix
        .is_some_and(|expires| expires <= now_unix() + threshold)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
#[path = "kimi_token_tests.rs"]
mod tests;
