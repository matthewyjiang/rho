use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    auth::anthropic_oauth::{CLIENT_ID, OAUTH_USER_AGENT, TOKEN_URL},
    credentials::{save_anthropic_tokens, AnthropicTokens, CredentialStore},
    model::ModelError,
};

const REFRESH_SKEW_SECONDS: i64 = 120;
static REFRESH_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnthropicAuthSource {
    Env,
    Store,
}

#[derive(Clone)]
pub(crate) struct AnthropicAuthMaterial {
    pub access_token: String,
}

impl std::fmt::Debug for AnthropicAuthMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AnthropicAuthMaterial")
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

pub struct AnthropicAuthManager {
    client: reqwest::Client,
    store: Arc<dyn CredentialStore>,
    source: AnthropicAuthSource,
    tokens: Mutex<AnthropicTokens>,
}

#[derive(Serialize)]
struct AnthropicRefreshRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Deserialize)]
struct AnthropicRefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

impl AnthropicAuthManager {
    pub(crate) fn from_tokens(
        store: Arc<dyn CredentialStore>,
        source: AnthropicAuthSource,
        tokens: AnthropicTokens,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            store,
            source,
            tokens: Mutex::new(tokens),
        }
    }

    pub(crate) async fn auth_material(&self) -> Result<AnthropicAuthMaterial, ModelError> {
        let tokens = self.tokens.lock().await.clone();
        if self.source == AnthropicAuthSource::Store && token_is_expiring(&tokens) {
            self.refresh_if_current(&tokens.access_token).await
        } else {
            Ok(AnthropicAuthMaterial {
                access_token: tokens.access_token,
            })
        }
    }

    pub(crate) async fn force_refresh(
        &self,
        failed_access_token: &str,
    ) -> Result<Option<AnthropicAuthMaterial>, ModelError> {
        if self.source != AnthropicAuthSource::Store {
            return Ok(None);
        }
        {
            let tokens = self.tokens.lock().await;
            if tokens.refresh_token.is_none() {
                return Ok(None);
            }
        }
        self.refresh_if_current(failed_access_token).await.map(Some)
    }

    async fn refresh_if_current(
        &self,
        failed_access_token: &str,
    ) -> Result<AnthropicAuthMaterial, ModelError> {
        let _guard = REFRESH_LOCK.lock().await;
        let mut current = self.tokens.lock().await;
        if current.access_token != failed_access_token {
            return Ok(AnthropicAuthMaterial {
                access_token: current.access_token.clone(),
            });
        }
        let refresh_token = current.refresh_token.clone().ok_or(
            crate::model::registry::missing_credentials_error("anthropic-oauth"),
        )?;
        let refreshed = refresh_anthropic_tokens(&self.client, &refresh_token).await?;
        save_anthropic_tokens(self.store.as_ref(), &refreshed)?;
        let access_token = refreshed.access_token.clone();
        *current = refreshed;
        Ok(AnthropicAuthMaterial { access_token })
    }
}

async fn refresh_anthropic_tokens(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<AnthropicTokens, ModelError> {
    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", OAUTH_USER_AGENT)
        .json(&AnthropicRefreshRequest {
            grant_type: "refresh_token",
            refresh_token,
            client_id: CLIENT_ID,
        })
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ModelError::HttpStatus {
            status,
            body,
            retry_after: None,
        });
    }
    let response = response.json::<AnthropicRefreshResponse>().await?;
    merge_refreshed_tokens(response, refresh_token, now_unix())
}

fn merge_refreshed_tokens(
    response: AnthropicRefreshResponse,
    previous_refresh_token: &str,
    now_unix: Option<i64>,
) -> Result<AnthropicTokens, ModelError> {
    let access_token = response.access_token.ok_or_else(|| {
        ModelError::InvalidResponse("Anthropic refresh response missing access_token".into())
    })?;
    Ok(AnthropicTokens {
        access_token,
        refresh_token: Some(
            response
                .refresh_token
                .unwrap_or_else(|| previous_refresh_token.to_string()),
        ),
        expires_at_unix: response.expires_in.and_then(|expires| {
            now_unix.and_then(|now| {
                i64::try_from(expires)
                    .ok()
                    .map(|expires| now.saturating_add(expires))
            })
        }),
    })
}

fn token_is_expiring(tokens: &AnthropicTokens) -> bool {
    tokens
        .expires_at_unix
        .zip(now_unix())
        .is_some_and(|(expires, now)| expires <= now.saturating_add(REFRESH_SKEW_SECONDS))
}

fn now_unix() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

#[cfg(test)]
#[path = "anthropic_token_tests.rs"]
mod tests;
