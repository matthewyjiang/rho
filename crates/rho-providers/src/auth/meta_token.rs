use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::meta_oauth::{
        mint_meta_tokens, now_unix, MetaOAuthError, API_KEY_MINT_URL, SESSION_EXPIRED,
    },
    credentials::{save_meta_tokens, CredentialStore, MetaTokens},
    model::{ModelError, ProviderReportedErrorKind},
};

/// Remint this long before the assumed 24 hour key lifetime.
const REFRESH_MARGIN_SECONDS: i64 = 300;

/// Stored Muse session.
///
/// A `META_API_KEY` override is a static Model API key, not a session. Callers
/// resolve that key before constructing this manager so it cannot be saved back
/// as an identity-less token.
pub struct MetaAuthManager {
    client: reqwest::Client,
    store: Arc<dyn CredentialStore>,
    tokens: Mutex<MetaTokens>,
    mint_url: String,
}

impl MetaAuthManager {
    pub(crate) fn from_tokens(store: Arc<dyn CredentialStore>, tokens: MetaTokens) -> Self {
        Self::from_tokens_at(store, tokens, API_KEY_MINT_URL)
    }

    fn from_tokens_at(
        store: Arc<dyn CredentialStore>,
        tokens: MetaTokens,
        mint_url: impl Into<String>,
    ) -> Self {
        Self {
            client: crate::reqwest_client(),
            store,
            tokens: Mutex::new(tokens),
            mint_url: mint_url.into(),
        }
    }

    pub(crate) async fn access_token(&self) -> Result<String, ModelError> {
        let mut tokens = self.tokens.lock().await;
        ensure_fresh(
            &self.client,
            self.store.as_ref(),
            &mut tokens,
            &self.mint_url,
        )
        .await?;
        Ok(tokens.api_key.clone())
    }

    pub(crate) async fn force_refresh(
        &self,
        rejected_key: &str,
    ) -> Result<Option<String>, ModelError> {
        let mut tokens = self.tokens.lock().await;
        if tokens.api_key != rejected_key {
            return Ok(Some(tokens.api_key.clone()));
        }
        refresh_locked(
            &self.client,
            self.store.as_ref(),
            &mut tokens,
            &self.mint_url,
        )
        .await?;
        Ok(Some(tokens.api_key.clone()))
    }
}

/// Non-blank `META_API_KEY`, when set. This key is not a Muse session.
pub(crate) fn env_api_key(env_var: &str) -> Option<String> {
    match std::env::var(env_var) {
        Ok(key) if !key.trim().is_empty() => Some(key),
        _ => None,
    }
}

pub(crate) fn meta_api_key_is_expiring(tokens: &MetaTokens, now: i64) -> bool {
    tokens.api_key_expires_at_unix <= now.saturating_add(REFRESH_MARGIN_SECONDS)
}

pub(crate) async fn ensure_fresh(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut MetaTokens,
    mint_url: &str,
) -> Result<(), ModelError> {
    let now = now_unix();
    if !meta_api_key_is_expiring(tokens, now) {
        return Ok(());
    }
    // The 24 hour lifetime is assumed. A key Meta has not expired yet still
    // works if the proactive mint is rate limited or blips.
    let still_accepted = tokens.api_key_expires_at_unix > now;
    match refresh_locked(client, store, tokens, mint_url).await {
        Ok(()) => Ok(()),
        Err(_) if still_accepted => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) async fn refresh_locked(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    tokens: &mut MetaTokens,
    mint_url: &str,
) -> Result<(), ModelError> {
    let refreshed = refresh_meta_tokens(client, tokens, mint_url).await?;
    save_meta_tokens(store, &refreshed)?;
    *tokens = refreshed;
    Ok(())
}

async fn refresh_meta_tokens(
    client: &reqwest::Client,
    tokens: &MetaTokens,
    mint_url: &str,
) -> Result<MetaTokens, ModelError> {
    let identity = tokens.identity_token.trim();
    if identity.is_empty() {
        return Err(missing());
    }
    mint_meta_tokens(client, identity, mint_url)
        .await
        .map_err(|error| match error {
            MetaOAuthError::Unauthorized(_) => ModelError::missing_credentials(SESSION_EXPIRED),
            MetaOAuthError::RateLimited => ModelError::ProviderReported {
                kind: ProviderReportedErrorKind::RateLimit,
                error_type: "rate_limit".into(),
                message: error.to_string(),
            },
            error => ModelError::InvalidResponse(error.to_string()),
        })
}

fn missing() -> ModelError {
    crate::model::registry::missing_credentials_error("meta-muse")
}

#[cfg(test)]
#[path = "meta_token_tests.rs"]
mod tests;
