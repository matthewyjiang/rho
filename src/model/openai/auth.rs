use std::path::PathBuf;

use serde::Deserialize;
use serde_json::json;

use crate::model::ModelError;

pub(super) enum Auth {
    ApiKey(String),
    Codex {
        access_token: String,
        refresh_token: Option<String>,
        account_id: Option<String>,
        auth_path: Option<PathBuf>,
    },
}

#[derive(Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokens>,
}
#[derive(Deserialize)]
pub(super) struct CodexTokens {
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) account_id: Option<String>,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

pub(super) fn load_codex_auth() -> Result<Auth, ModelError> {
    if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
        let account_id = std::env::var("CODEX_ACCOUNT_ID").ok();
        return Ok(Auth::Codex {
            access_token,
            refresh_token: None,
            account_id,
            auth_path: None,
        });
    }
    let home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .map_err(|_| ModelError::MissingCodexAuth)?;
    let auth_path = home.join("auth.json");
    let tokens = load_codex_tokens_from_path(&auth_path)?;
    Ok(Auth::Codex {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        account_id: tokens.account_id,
        auth_path: Some(auth_path),
    })
}

pub(super) fn load_codex_tokens_from_path(
    path: &std::path::Path,
) -> Result<CodexTokens, ModelError> {
    let text = std::fs::read_to_string(path).map_err(|_| ModelError::MissingCodexAuth)?;
    let file: CodexAuthFile = serde_json::from_str(&text)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid Codex auth.json: {e}")))?;
    file.tokens.ok_or(ModelError::MissingCodexAuth)
}

pub(super) async fn refresh_codex_token(
    client: &reqwest::Client,
    refresh_token: &str,
    auth_path: Option<&std::path::Path>,
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
    let new_refresh_token = response
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string());
    let mut account_id = None;

    if let Some(path) = auth_path {
        let text = std::fs::read_to_string(path)?;
        let mut value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| ModelError::InvalidResponse(format!("invalid Codex auth.json: {e}")))?;
        if let Some(tokens) = value.get_mut("tokens") {
            if let Some(obj) = tokens.as_object_mut() {
                obj.insert("access_token".into(), json!(access_token));
                obj.insert("refresh_token".into(), json!(new_refresh_token));
                if let Some(id_token) = response.id_token {
                    obj.insert("id_token".into(), json!(id_token));
                }
                account_id = obj
                    .get("account_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
        }
        std::fs::write(path, serde_json::to_string_pretty(&value).unwrap())?;
    }

    Ok(CodexTokens {
        access_token,
        refresh_token: Some(new_refresh_token),
        account_id,
    })
}
