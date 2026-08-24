use serde::Deserialize;

use crate::{
    credentials::{save_codex_tokens, CodexTokens, CredentialStore},
    model::ModelError,
};

pub enum Auth {
    ApiKey(String),
    Codex {
        tokens: CodexTokens,
        source: CodexAuthSource,
    },
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

pub(super) async fn refresh_codex_token_at(
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
