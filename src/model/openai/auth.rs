use serde::Deserialize;

use crate::{
    credentials::{
        load_codex_tokens, load_provider_api_key, save_codex_tokens, CodexTokens, OsCredentialStore,
    },
    model::{registry, ModelError},
};

pub(crate) enum Auth {
    ApiKey(String),
    Codex {
        tokens: CodexTokens,
        source: CodexAuthSource,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodexAuthSource {
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

pub(crate) fn load_api_key_auth() -> Result<Auth, ModelError> {
    let descriptor = registry::provider_descriptor("openai")
        .ok_or_else(|| ModelError::UnsupportedProvider("openai".into()))?;
    let registry::ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("openai".into()));
    };
    if let Ok(key) = std::env::var(env_var) {
        return Ok(Auth::ApiKey(key));
    }
    let store = OsCredentialStore;
    let key = load_provider_api_key(&store, descriptor.name)?
        .ok_or_else(|| registry::missing_credential_error(missing))?;
    Ok(Auth::ApiKey(key))
}

pub(crate) fn load_codex_auth() -> Result<Auth, ModelError> {
    if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
        return Ok(Auth::Codex {
            tokens: CodexTokens {
                access_token,
                refresh_token: None,
                id_token: None,
                account_id: std::env::var("CODEX_ACCOUNT_ID").ok(),
            },
            source: CodexAuthSource::Env,
        });
    }
    let store = OsCredentialStore;
    let tokens = load_codex_tokens(&store)?.ok_or(ModelError::MissingCodexAuth)?;
    Ok(Auth::Codex {
        tokens,
        source: CodexAuthSource::Store,
    })
}

pub(crate) fn load_codex_tokens_for_request(
    tokens: &CodexTokens,
    source: CodexAuthSource,
) -> Result<CodexTokens, ModelError> {
    match source {
        CodexAuthSource::Env => Ok(tokens.clone()),
        CodexAuthSource::Store => {
            let store = OsCredentialStore;
            load_codex_tokens(&store)?.ok_or(ModelError::MissingCodexAuth)
        }
    }
}

pub(crate) async fn refresh_codex_token(
    client: &reqwest::Client,
    refresh_token: &str,
    source: CodexAuthSource,
    previous: &CodexTokens,
) -> Result<CodexTokens, ModelError> {
    let response: RefreshResponse = client
        .post("https://auth.openai.com/oauth/token")
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
        let store = OsCredentialStore;
        save_codex_tokens(&store, &refreshed)?;
    }

    Ok(refreshed)
}
