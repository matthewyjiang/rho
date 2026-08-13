use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{StatusCode, Url};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    auth::github_copilot_token::{
        auth_material_with_store, force_refresh_auth_material_with_store,
        GitHubCopilotAuthMaterial, GitHubCopilotAuthSource,
    },
    credentials::{load_provider_api_key, CredentialStore},
    model::{registry::missing_credential_error, ModelError, ReasoningCapabilities},
    provider::{self, ProviderAuthKind, ProviderModelRefreshKind},
};

#[cfg(not(test))]
use crate::paths;

#[path = "provider_models/anthropic.rs"]
mod anthropic;
#[path = "provider_models/google.rs"]
mod google;
pub(crate) use google::{thinking_policy, ThinkingPolicy};
#[path = "provider_models/kimi_capabilities.rs"]
mod kimi_capabilities;
#[path = "provider_models/openai_compatible.rs"]
mod openai_compatible;
pub use openai_compatible::probe_provider_models;

pub(super) struct ProviderModelRecord {
    pub model: ProviderModel,
    pub raw_json: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub reasoning_capabilities: ReasoningCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderModelHealth {
    ReachableWithModels { model_count: usize },
    ReachableWithoutModels,
    Unreachable { error: String },
    InvalidResponse { error: String },
}

/// Endpoint data for provider model discovery.
///
/// Provider-owned protocols use their built-in endpoint policy. OpenAI-compatible discovery
/// requires the application to pass the same resolved API base used for runtime construction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderModelEndpoint<'a> {
    #[default]
    ProviderOwned,
    OpenAiCompatible(&'a Url),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModelRefresh {
    pub provider: String,
    pub models: Vec<ProviderModel>,
}

pub fn cached_provider_model(provider: &str, model: &str) -> Option<ProviderModel> {
    let model = provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.canonicalize_model_id(model))
        .unwrap_or_else(|| model.to_string());
    cached_provider_models(provider)
        .into_iter()
        .find(|entry| entry.model == model)
}

pub(crate) fn cached_provider_model_raw_json(provider: &str, model: &str) -> Option<Value> {
    let model = provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.canonicalize_model_id(model))
        .unwrap_or_else(|| model.to_string());
    let connection = open_provider_models_cache().ok()?;
    let raw: Option<String> = connection
        .query_row(
            "select raw_json from provider_models where provider = ?1 and model = ?2",
            params![provider, model],
            |row| row.get(0),
        )
        .ok()?;
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

/// Replaces the provider's cached rows with one model carrying `raw_json`,
/// through the same write path production refreshes use.
#[cfg(test)]
pub(crate) fn write_cached_provider_model_raw_json_for_tests(
    provider: &str,
    model: &str,
    display_name: &str,
    raw_json: &Value,
) -> Result<(), ModelError> {
    replace_cached_provider_model_records(
        provider,
        &[ProviderModelRecord {
            model: ProviderModel {
                provider: provider.to_string(),
                model: model.to_string(),
                display_name: display_name.to_string(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            },
            raw_json: raw_json.clone(),
        }],
    )
}

pub fn cached_provider_models(provider: &str) -> Vec<ProviderModel> {
    let Ok(connection) = open_provider_models_cache() else {
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare(
        "select model, display_name, context_window, max_output_tokens, reasoning_capabilities_json from provider_models where provider = ?1 order by model",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![provider], |row| {
        let model: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        let context_window: Option<u64> = row.get(2)?;
        let max_output_tokens: Option<u64> = row.get(3)?;
        let reasoning_capabilities = row
            .get::<_, Option<String>>(4)?
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        let model = provider::provider_descriptor(provider)
            .map(|descriptor| descriptor.canonicalize_model_id(&model))
            .unwrap_or(model);
        Ok(ProviderModel {
            provider: provider.to_string(),
            model,
            display_name,
            context_window,
            max_output_tokens,
            reasoning_capabilities,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

const PROVIDER_MODEL_CACHE_VERSION: i64 = 2;
const PROVIDER_MODEL_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub fn provider_model_capabilities_need_refresh(provider: &str, model: &str) -> bool {
    let capabilities_are_known = match provider {
        "kimi-code" => kimi_capabilities_are_known as fn(&CachedCapabilityRow) -> bool,
        "anthropic" => anthropic_capabilities_are_known,
        _ => return false,
    };
    let Some(row) = cached_capability_row(provider, model) else {
        return true;
    };
    row.cache_version < PROVIDER_MODEL_CACHE_VERSION
        || !capabilities_are_known(&row)
        || !row
            .updated_at
            .is_some_and(provider_snapshot_timestamp_is_fresh)
}

struct CachedCapabilityRow {
    cache_version: i64,
    reasoning_capabilities_json: Option<String>,
    raw_json: Option<String>,
    updated_at: Option<i64>,
}

fn cached_capability_row(provider: &str, model: &str) -> Option<CachedCapabilityRow> {
    let connection = open_provider_models_cache().ok()?;
    connection
        .query_row(
            "select models.cache_version, models.reasoning_capabilities_json, models.raw_json, refresh.updated_at
             from provider_models models
             left join provider_model_refresh refresh on refresh.provider = models.provider
             where models.provider = ?1 and models.model = ?2",
            params![provider, model],
            |row| {
                Ok(CachedCapabilityRow {
                    cache_version: row.get(0)?,
                    reasoning_capabilities_json: row.get(1)?,
                    raw_json: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .ok()
}

fn kimi_capabilities_are_known(row: &CachedCapabilityRow) -> bool {
    row.reasoning_capabilities_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<ReasoningCapabilities>(value).ok())
        .unwrap_or_default()
        .is_known()
}

// Anthropic capabilities live in raw_json as the Models API `capabilities`
// object; anything but a JSON object (missing row, legacy null) is unknown.
fn anthropic_capabilities_are_known(row: &CachedCapabilityRow) -> bool {
    row.raw_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .is_some_and(|value| value.is_object())
}

fn provider_snapshot_timestamp_is_fresh(updated_at: i64) -> bool {
    let Ok(max_age) = i64::try_from(PROVIDER_MODEL_MAX_AGE.as_secs()) else {
        return false;
    };
    let Some(now) = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
    else {
        return false;
    };
    updated_at <= now && now - updated_at <= max_age
}

pub async fn refresh_provider_models_with_store(
    provider: &str,
    auth: &str,
    store: &dyn CredentialStore,
    endpoint: ProviderModelEndpoint<'_>,
) -> Result<ProviderModelRefresh, ModelError> {
    let profile = provider::resolve_profile(provider, auth)
        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
    let descriptor = profile.provider;
    let provider = descriptor.name;
    let auth_mode = profile.auth;
    let records = match descriptor.model_refresh {
        Some(ProviderModelRefreshKind::OpenAi) => {
            records_from_models(fetch_openai_models(provider, store).await?)
        }
        Some(ProviderModelRefreshKind::Anthropic) => anthropic::fetch(provider, store).await?,
        Some(ProviderModelRefreshKind::Google) => {
            records_from_models(google::fetch(provider, load_api_key_auth(provider, store)?).await?)
        }
        Some(ProviderModelRefreshKind::GithubCopilot) => {
            records_from_models(fetch_github_copilot_models(provider, store).await?)
        }
        Some(ProviderModelRefreshKind::OpenAiCompatible) => {
            let ProviderModelEndpoint::OpenAiCompatible(api_base) = endpoint else {
                return Err(ModelError::InvalidResponse(format!(
                    "provider '{}' requires a resolved API base for model discovery",
                    descriptor.name
                )));
            };
            records_from_models(
                openai_compatible::fetch(descriptor, auth_mode, api_base, store).await?,
            )
        }
        None => return Err(ModelError::UnsupportedProvider(provider.to_string())),
    };
    replace_cached_provider_model_records(provider, &records)?;
    Ok(ProviderModelRefresh {
        provider: provider.to_string(),
        models: records.into_iter().map(|record| record.model).collect(),
    })
}

fn records_from_models(models: Vec<ProviderModel>) -> Vec<ProviderModelRecord> {
    models
        .into_iter()
        .map(|model| ProviderModelRecord {
            model,
            raw_json: Value::Null,
        })
        .collect()
}

fn replace_cached_provider_models(
    provider: &str,
    models: &[ProviderModel],
) -> Result<(), ModelError> {
    replace_cached_provider_model_records(provider, &records_from_models(models.to_vec()))
}

fn replace_cached_provider_model_records(
    provider: &str,
    records: &[ProviderModelRecord],
) -> Result<(), ModelError> {
    let mut connection = open_provider_models_cache().map_err(model_cache_error)?;
    let tx = connection.transaction().map_err(model_cache_error)?;
    tx.execute(
        "delete from provider_models where provider = ?1",
        params![provider],
    )
    .map_err(model_cache_error)?;
    for record in records {
        let reasoning_capabilities = serde_json::to_string(&record.model.reasoning_capabilities)
            .map_err(|error| {
                ModelError::InvalidResponse(format!(
                    "failed to serialize provider reasoning capabilities: {error}"
                ))
            })?;
        tx.execute(
            "insert into provider_models (provider, model, display_name, context_window, max_output_tokens, reasoning_capabilities_json, cache_version, raw_json, updated_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%s', 'now'))",
            params![
                provider,
                record.model.model,
                record.model.display_name,
                record.model.context_window,
                record.model.max_output_tokens,
                reasoning_capabilities,
                PROVIDER_MODEL_CACHE_VERSION,
                record.raw_json.to_string()
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
    super::display_name::forget_provider_display_names(provider);
    Ok(())
}

async fn fetch_openai_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
    let response: OpenAiModelsResponse = provider_models_client()?
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
            provider: provider.to_string(),
            display_name: model.display_name.unwrap_or_else(|| model.id.clone()),
            context_window: model.context_length.filter(|window| *window > 0),
            model: model.id,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

async fn fetch_github_copilot_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = provider_models_client()?;
    let auth = auth_material_with_store(&client, store).await?;
    let response = send_github_copilot_models_request(&client, &auth).await?;
    let response = if response.status() == StatusCode::UNAUTHORIZED
        && auth.source == GitHubCopilotAuthSource::Store
    {
        if let Some(refreshed) = force_refresh_auth_material_with_store(&client, store).await? {
            send_github_copilot_models_request(&client, &refreshed).await?
        } else {
            response
        }
    } else {
        response
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return if status == StatusCode::UNAUTHORIZED {
            Err(crate::model::registry::missing_credentials_error(
                "github-copilot",
            ))
        } else {
            Err(ModelError::HttpStatus {
                status,
                body,
                retry_after: None,
            })
        };
    }
    let value = response.json::<Value>().await?;
    parse_github_copilot_models(provider, &value)
}

async fn send_github_copilot_models_request(
    client: &reqwest::Client,
    auth: &GitHubCopilotAuthMaterial,
) -> Result<reqwest::Response, ModelError> {
    Ok(client
        .get(&auth.models_endpoint)
        .bearer_auth(&auth.token)
        .header("Accept", "application/json")
        .header("User-Agent", crate::rho_user_agent())
        .header("Editor-Version", crate::rho_user_agent())
        .header("Editor-Plugin-Version", crate::rho_user_agent())
        .header("Copilot-Integration-Id", "vscode-chat")
        .send()
        .await?)
}

fn parse_github_copilot_models(
    provider: &str,
    value: &Value,
) -> Result<Vec<ProviderModel>, ModelError> {
    let raw_models = value
        .get("data")
        .or_else(|| value.get("models"))
        .unwrap_or(value)
        .as_array()
        .ok_or_else(|| {
            ModelError::InvalidResponse("GitHub Copilot models response was not an array".into())
        })?;
    let mut models = raw_models
        .iter()
        .filter_map(|value| {
            value.as_str().map(ToOwned::to_owned).or_else(|| {
                value
                    .get("id")
                    .or_else(|| value.get("name"))
                    .and_then(|id| id.as_str())
                    .map(ToOwned::to_owned)
            })
        })
        .filter(|model| !model.trim().is_empty())
        .map(|model| ProviderModel {
            provider: provider.to_string(),
            display_name: model.clone(),
            model,
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

fn provider_models_client() -> Result<reqwest::Client, ModelError> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?)
}

fn load_api_key_auth(provider: &str, store: &dyn CredentialStore) -> Result<String, ModelError> {
    let descriptor = provider::provider_descriptor(provider)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
    let ProviderAuthKind::ApiKey {
        env_var,
        missing_message,
        ..
    } = descriptor.default_auth().auth_kind
    else {
        return Err(ModelError::UnsupportedProvider(provider.to_string()));
    };
    if let Ok(key) = std::env::var(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    load_provider_api_key(store, provider)?.ok_or_else(|| missing_credential_error(missing_message))
}

fn is_supported_openai_model(model: &str) -> bool {
    let is_reasoning =
        model.starts_with('o') && model.chars().nth(1).is_some_and(|c| c.is_ascii_digit());
    let is_gpt = model.starts_with("gpt-")
        && !model.contains("realtime")
        && !model.contains("audio")
        && !model.contains("image");
    is_reasoning || is_gpt
}

#[derive(Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(alias = "name")]
    display_name: Option<String>,
    context_length: Option<u64>,
    #[serde(flatten)]
    kimi_reasoning: kimi_capabilities::KimiReasoningMetadata,
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
            context_window integer,
            max_output_tokens integer,
            reasoning_capabilities_json text,
            cache_version integer not null default 1,
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
        "alter table provider_models add column context_window integer",
        [],
    );
    let _ = connection.execute(
        "alter table provider_models add column max_output_tokens integer",
        [],
    );
    let _ = connection.execute(
        "alter table provider_models add column reasoning_capabilities_json text",
        [],
    );
    let _ = connection.execute(
        "alter table provider_models add column cache_version integer not null default 1",
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
    if let Some(path) = test_cache_dir() {
        return path;
    }
    #[cfg(test)]
    return default_test_cache_dir();
    #[cfg(not(test))]
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("rho");
    }
    #[cfg(not(test))]
    {
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
}

thread_local! {
    static TEST_CACHE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn test_cache_dir() -> Option<PathBuf> {
    TEST_CACHE_DIR.with(|path| path.borrow().clone())
}

#[cfg(test)]
fn default_test_cache_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rho-provider-models-default-test-cache-{}",
        std::process::id()
    ))
}

#[doc(hidden)]
pub fn with_provider_models_cache_dir_for_tests<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    TEST_CACHE_DIR.with(|cache_dir| {
        let previous = cache_dir.replace(Some(path));
        let result = f();
        cache_dir.replace(previous);
        result
    })
}

#[doc(hidden)]
pub fn set_provider_models_cache_dir_for_tests(path: Option<PathBuf>) {
    TEST_CACHE_DIR.with(|cache_dir| {
        cache_dir.replace(path);
    });
}

#[doc(hidden)]
pub fn replace_cached_provider_models_for_tests(
    provider: &str,
    models: &[ProviderModel],
) -> Result<(), ModelError> {
    replace_cached_provider_models(provider, models)
}

#[cfg(test)]
fn unique_test_cache_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock should be after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rho-provider-models-{name}-{}-{nanos}",
        std::process::id()
    ))
}

#[cfg(test)]
#[path = "provider_models_capabilities_tests.rs"]
mod capability_tests;

#[cfg(test)]
#[path = "provider_models/provider_models_tests.rs"]
mod tests;
