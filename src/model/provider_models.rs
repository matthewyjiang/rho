use std::{fs, path::PathBuf};

use reqwest::Url;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    credentials::{load_anthropic_api_key, load_openai_api_key, OsCredentialStore},
    model::ModelError,
    paths,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub max_output_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModelRefresh {
    pub provider: String,
    pub models: Vec<ProviderModel>,
}

pub fn cached_provider_model(provider: &str, model: &str) -> Option<ProviderModel> {
    cached_provider_models(provider)
        .into_iter()
        .find(|entry| entry.model == model)
}

#[cfg(test)]
pub fn cached_provider_models(_provider: &str) -> Vec<ProviderModel> {
    Vec::new()
}

#[cfg(not(test))]
pub fn cached_provider_models(provider: &str) -> Vec<ProviderModel> {
    let Ok(connection) = open_provider_models_cache() else {
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare(
        "select model, display_name, max_output_tokens from provider_models where provider = ?1 order by model",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![provider], |row| {
        let model: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        let max_output_tokens: Option<u64> = row.get(2)?;
        Ok(ProviderModel {
            provider: provider.to_string(),
            model,
            display_name,
            max_output_tokens,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

pub async fn refresh_provider_models(provider: &str) -> Result<ProviderModelRefresh, ModelError> {
    let models = match provider {
        "openai" => fetch_openai_models().await?,
        "anthropic" => fetch_anthropic_models().await?,
        other => return Err(ModelError::UnsupportedProvider(other.to_string())),
    };
    replace_cached_provider_models(provider, &models)?;
    Ok(ProviderModelRefresh {
        provider: provider.to_string(),
        models,
    })
}

fn replace_cached_provider_models(
    provider: &str,
    models: &[ProviderModel],
) -> Result<(), ModelError> {
    let mut connection = open_provider_models_cache().map_err(model_cache_error)?;
    let tx = connection.transaction().map_err(model_cache_error)?;
    tx.execute(
        "delete from provider_models where provider = ?1",
        params![provider],
    )
    .map_err(model_cache_error)?;
    for model in models {
        tx.execute(
            "insert into provider_models (provider, model, display_name, max_output_tokens, raw_json, updated_at)
             values (?1, ?2, ?3, ?4, ?5, strftime('%s', 'now'))",
            params![
                provider,
                model.model,
                model.display_name,
                model.max_output_tokens,
                Value::Null.to_string()
            ],
        )
        .map_err(model_cache_error)?;
    }
    tx.execute(
        "insert into provider_model_refresh (provider, updated_at, error)
         values (?1, strftime('%s', 'now'), null)
         on conflict(provider) do update set updated_at = excluded.updated_at, error = null",
        params![provider],
    )
    .map_err(model_cache_error)?;
    tx.commit().map_err(model_cache_error)?;
    Ok(())
}

async fn fetch_openai_models() -> Result<Vec<ProviderModel>, ModelError> {
    let key = load_openai_api_key_auth()?;
    let response: OpenAiModelsResponse = reqwest::Client::new()
        .get("https://api.openai.com/v1/models")
        .bearer_auth(key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut models = response
        .data
        .into_iter()
        .filter(|model| is_supported_openai_model(&model.id))
        .map(|model| ProviderModel {
            provider: "openai".to_string(),
            display_name: model.id.clone(),
            model: model.id,
            max_output_tokens: None,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

async fn fetch_anthropic_models() -> Result<Vec<ProviderModel>, ModelError> {
    let key = load_anthropic_api_key_auth()?;
    let client = reqwest::Client::new();
    let mut models = Vec::new();
    let mut after_id = None::<String>;
    loop {
        let mut url = Url::parse("https://api.anthropic.com/v1/models").map_err(|err| {
            ModelError::InvalidResponse(format!("invalid Anthropic models URL: {err}"))
        })?;
        if let Some(after_id) = &after_id {
            url.query_pairs_mut().append_pair("after_id", after_id);
        }
        let response: AnthropicModelsResponse = client
            .get(url)
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let last_id = response.last_id.clone();
        models.extend(
            response
                .data
                .into_iter()
                .filter(|model| model.id.starts_with("claude-"))
                .map(|model| ProviderModel {
                    provider: "anthropic".to_string(),
                    display_name: model.display_name.unwrap_or_else(|| model.id.clone()),
                    model: model.id,
                    max_output_tokens: model.max_tokens,
                }),
        );
        if !response.has_more {
            break;
        }
        let Some(next_after_id) = last_id else {
            break;
        };
        after_id = Some(next_after_id);
    }
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

fn load_openai_api_key_auth() -> Result<String, ModelError> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(key);
    }
    let store = OsCredentialStore;
    load_openai_api_key(&store)?.ok_or(ModelError::MissingApiKey)
}

fn load_anthropic_api_key_auth() -> Result<String, ModelError> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        return Ok(key);
    }
    let store = OsCredentialStore;
    load_anthropic_api_key(&store)?.ok_or(ModelError::MissingAnthropicApiKey)
}

fn is_supported_openai_model(model: &str) -> bool {
    model.starts_with("gpt-") || model.starts_with('o')
}

#[derive(Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

#[derive(Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
    max_tokens: Option<u64>,
}

fn open_provider_models_cache() -> rusqlite::Result<Connection> {
    let path = provider_models_sqlite_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "create table if not exists provider_models (
            provider text not null,
            model text not null,
            display_name text not null,
            max_output_tokens integer,
            raw_json text,
            updated_at integer not null,
            primary key(provider, model)
        );
        create table if not exists provider_model_refresh (
            provider text primary key,
            updated_at integer not null,
            error text
        );",
    )?;
    let _ = connection.execute(
        "alter table provider_models add column max_output_tokens integer",
        [],
    );
    Ok(connection)
}

fn model_cache_error(error: rusqlite::Error) -> ModelError {
    ModelError::InvalidResponse(format!("provider model cache error: {error}"))
}

fn provider_models_sqlite_path() -> PathBuf {
    cache_dir().join("provider-models.sqlite3")
}

fn cache_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("rho");
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(path).join("rho").join("cache");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = paths::home_dir() {
            return path.join("Library").join("Caches").join("rho");
        }
    }
    if let Some(path) = paths::home_dir() {
        return path.join(".cache").join("rho");
    }
    std::env::temp_dir().join("rho-cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_model_filter_keeps_chat_families() {
        assert!(is_supported_openai_model("gpt-5.5"));
        assert!(is_supported_openai_model("o3"));
        assert!(!is_supported_openai_model("text-embedding-3-large"));
        assert!(!is_supported_openai_model("whisper-1"));
    }
}
