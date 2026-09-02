use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::{
    credentials::{load_codex_tokens, save_codex_tokens, CodexTokens, CredentialStore},
    model::ModelError,
};

pub enum Auth {
    ApiKey(String),
    Codex {
        tokens: CodexTokens,
        source: CodexAuthSource,
        refresh_store: Arc<dyn CredentialStore>,
        refreshed_tokens: Mutex<Option<CodexTokens>>,
    },
}

impl Auth {
    pub(crate) fn codex(
        tokens: CodexTokens,
        source: CodexAuthSource,
        refresh_store: Arc<dyn CredentialStore>,
    ) -> Self {
        Self::Codex {
            tokens,
            source,
            refresh_store,
            refreshed_tokens: Mutex::new(None),
        }
    }

    pub(super) fn codex_tokens_for_auth(auth: Option<&Self>) -> Result<CodexTokens, ModelError> {
        auth.ok_or_else(|| {
            ModelError::InvalidResponse("Codex tokens requested for non-Codex auth".into())
        })?
        .codex_tokens_for_request()
    }

    pub(crate) fn codex_tokens_for_request(&self) -> Result<CodexTokens, ModelError> {
        let Self::Codex {
            tokens,
            source,
            refresh_store,
            refreshed_tokens,
        } = self
        else {
            return Err(ModelError::InvalidResponse(
                "Codex tokens requested for non-Codex auth".into(),
            ));
        };
        if *source == CodexAuthSource::Store {
            if let Ok(Some(stored)) = load_codex_tokens(refresh_store.as_ref()) {
                return Ok(stored);
            }
        }
        Ok(refreshed_tokens
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| tokens.clone()))
    }

    pub(crate) fn remember_refreshed_codex_tokens(&self, tokens: CodexTokens) {
        let Self::Codex {
            refreshed_tokens, ..
        } = self
        else {
            return;
        };
        if let Ok(mut guard) = refreshed_tokens.lock() {
            *guard = Some(tokens);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexAuthSource {
    Env,
    Store,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    account_id: Option<String>,
}

pub async fn refresh_codex_token(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    refresh_token: &str,
    source: CodexAuthSource,
    previous: &CodexTokens,
) -> Result<CodexTokens, ModelError> {
    refresh_codex_token_at(
        client,
        store,
        refresh_token,
        source,
        previous,
        "https://auth.openai.com/oauth/token",
    )
    .await
}

pub(crate) async fn refresh_codex_token_at(
    client: &reqwest::Client,
    store: &dyn CredentialStore,
    refresh_token: &str,
    source: CodexAuthSource,
    previous: &CodexTokens,
    token_url: &str,
) -> Result<CodexTokens, ModelError> {
    let response: RefreshResponse = client
        .post(token_url)
        .form(&[
            ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let access_token = response.access_token.ok_or_else(|| {
        ModelError::InvalidResponse("refresh response missing access_token".into())
    })?;
    let refreshed = CodexTokens {
        access_token,
        refresh_token: Some(
            response
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_string()),
        ),
        id_token: response.id_token.or_else(|| previous.id_token.clone()),
        account_id: response.account_id.or_else(|| previous.account_id.clone()),
    };

    if source == CodexAuthSource::Store {
        save_codex_tokens(store, &refreshed)?;
    }

    Ok(refreshed)
}
