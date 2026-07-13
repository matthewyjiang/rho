use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{
    auth::xai_oauth::{CLIENT_ID, TOKEN_URL},
    credentials::{load_xai_tokens, save_xai_tokens, CredentialStore, XaiTokens},
    model::ModelError,
    provider::{self, ProviderId},
};

const REFRESH_SKEW_SECONDS: i64 = 120;
static REFRESH_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XaiAuthSource {
    Env,
    Store,
}

#[derive(Clone, Debug)]
pub(crate) struct XaiAuthMaterial {
    pub access_token: String,
}

pub(crate) struct XaiAuthManager {
    client: reqwest::Client,
    store: Arc<dyn CredentialStore>,
    source: XaiAuthSource,
    env_tokens: Option<XaiTokens>,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

impl XaiAuthManager {
    pub(crate) fn new(store: Arc<dyn CredentialStore>) -> Result<Self, ModelError> {
        let descriptor = provider::provider_descriptor_by_id(ProviderId::Xai);
        let (source, env_tokens) = match std::env::var(descriptor.auth_kind.env_var()) {
            Ok(access_token) if !access_token.trim().is_empty() => (
                XaiAuthSource::Env,
                Some(XaiTokens {
                    access_token,
                    refresh_token: None,
                    expires_at_unix: None,
                    id_token: None,
                }),
            ),
            _ => {
                load_xai_tokens(store.as_ref())?.ok_or(ModelError::MissingXaiAuth)?;
                (XaiAuthSource::Store, None)
            }
        };
        Ok(Self {
            client: reqwest::Client::new(),
            store,
            source,
            env_tokens,
        })
    }

    pub(crate) async fn auth_material(&self) -> Result<XaiAuthMaterial, ModelError> {
        let tokens = self.load_tokens()?;
        if self.source == XaiAuthSource::Store && token_is_expiring(&tokens) {
            self.refresh_if_current(&tokens.access_token).await
        } else {
            Ok(XaiAuthMaterial {
                access_token: tokens.access_token,
            })
        }
    }

    pub(crate) async fn force_refresh(
        &self,
        failed_access_token: &str,
    ) -> Result<Option<XaiAuthMaterial>, ModelError> {
        if self.source == XaiAuthSource::Env {
            return Ok(None);
        }
        self.refresh_if_current(failed_access_token).await.map(Some)
    }

    fn load_tokens(&self) -> Result<XaiTokens, ModelError> {
        match self.source {
            XaiAuthSource::Env => self.env_tokens.clone().ok_or(ModelError::MissingXaiAuth),
            XaiAuthSource::Store => {
                load_xai_tokens(self.store.as_ref())?.ok_or(ModelError::MissingXaiAuth)
            }
        }
    }

    async fn refresh_if_current(
        &self,
        failed_access_token: &str,
    ) -> Result<XaiAuthMaterial, ModelError> {
        let _guard = REFRESH_LOCK.lock().await;
        let current = self.load_tokens()?;
        if current.access_token != failed_access_token {
            return Ok(XaiAuthMaterial {
                access_token: current.access_token,
            });
        }
        let refresh_token = current
            .refresh_token
            .clone()
            .ok_or(ModelError::MissingXaiAuth)?;
        let refreshed = refresh_tokens(&self.client, &refresh_token, &current).await?;
        save_xai_tokens(self.store.as_ref(), &refreshed)?;
        Ok(XaiAuthMaterial {
            access_token: refreshed.access_token,
        })
    }
}

async fn refresh_tokens(
    client: &reqwest::Client,
    refresh_token: &str,
    previous: &XaiTokens,
) -> Result<XaiTokens, ModelError> {
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ModelError::HttpStatus { status, body });
    }
    let response = response.json::<RefreshResponse>().await?;
    merge_refreshed_tokens(response, refresh_token, previous, now_unix())
}

fn merge_refreshed_tokens(
    response: RefreshResponse,
    previous_refresh_token: &str,
    previous: &XaiTokens,
    now_unix: Option<i64>,
) -> Result<XaiTokens, ModelError> {
    let access_token = response.access_token.ok_or_else(|| {
        ModelError::InvalidResponse("xAI refresh response missing access_token".into())
    })?;
    Ok(XaiTokens {
        access_token,
        refresh_token: Some(
            response
                .refresh_token
                .unwrap_or_else(|| previous_refresh_token.to_string()),
        ),
        // Clear expiry when the refresh response omits expires_in. Carrying the
        // previous timestamp forward can leave an already-stale expiry and force
        // a refresh on every subsequent request.
        expires_at_unix: response.expires_in.and_then(|expires| {
            now_unix.and_then(|now| {
                i64::try_from(expires)
                    .ok()
                    .map(|expires| now.saturating_add(expires))
            })
        }),
        id_token: response.id_token.or_else(|| previous.id_token.clone()),
    })
}

fn token_is_expiring(tokens: &XaiTokens) -> bool {
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
#[path = "xai_token_tests.rs"]
mod tests;
