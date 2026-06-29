use std::{fs, path::PathBuf};

#[cfg(test)]
use std::{
    cell::RefCell,
    time::{SystemTime, UNIX_EPOCH},
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
    model::{
        registry::{self, missing_credential_error, ProviderAuthKind, ProviderModelRefreshKind},
        ModelError,
    },
};

#[cfg(not(test))]
use crate::paths;

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

pub async fn refresh_provider_models_with_store(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<ProviderModelRefresh, ModelError> {
    let descriptor = registry::provider_descriptor(provider)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
    let models = match descriptor.model_refresh {
        Some(ProviderModelRefreshKind::OpenAi) => fetch_openai_models(provider, store).await?,
        Some(ProviderModelRefreshKind::Anthropic) => {
            fetch_anthropic_models(provider, store).await?
        }
        Some(ProviderModelRefreshKind::GithubCopilot) => {
            fetch_github_copilot_models(provider, store).await?
        }
        None => return Err(ModelError::UnsupportedProvider(provider.to_string())),
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

async fn fetch_openai_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
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
            provider: provider.to_string(),
            display_name: model.id.clone(),
            model: model.id,
            max_output_tokens: None,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

async fn fetch_anthropic_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
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
                    provider: provider.to_string(),
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

async fn fetch_github_copilot_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = reqwest::Client::new();
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
            Err(ModelError::MissingGithubCopilotAuth)
        } else {
            Err(ModelError::HttpStatus { status, body })
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
        .header("User-Agent", concat!("rho/", env!("CARGO_PKG_VERSION")))
        .header("Editor-Version", concat!("rho/", env!("CARGO_PKG_VERSION")))
        .header(
            "Editor-Plugin-Version",
            concat!("rho/", env!("CARGO_PKG_VERSION")),
        )
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
            max_output_tokens: None,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

fn load_api_key_auth(provider: &str, store: &dyn CredentialStore) -> Result<String, ModelError> {
    let descriptor = registry::provider_descriptor(provider)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
    let ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider(provider.to_string()));
    };
    if let Ok(key) = std::env::var(env_var) {
        return Ok(key);
    }
    load_provider_api_key(store, provider)?.ok_or_else(|| missing_credential_error(missing))
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
    #[cfg(test)]
    {
        if let Some(path) = test_cache_dir() {
            return path;
        }
        default_test_cache_dir()
    }
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

#[cfg(test)]
thread_local! {
    static TEST_CACHE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
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

#[cfg(test)]
pub fn with_provider_models_cache_dir_for_tests<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    TEST_CACHE_DIR.with(|cache_dir| {
        let previous = cache_dir.replace(Some(path));
        let result = f();
        cache_dir.replace(previous);
        result
    })
}

#[cfg(test)]
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
mod tests {
    use super::*;
    use crate::credentials::{
        save_github_copilot_tokens, save_provider_api_key, GitHubCopilotTokens,
        MemoryCredentialStore,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn openai_model_filter_keeps_chat_families() {
        assert!(is_supported_openai_model("gpt-5.5"));
        assert!(is_supported_openai_model("o3"));
        assert!(!is_supported_openai_model("text-embedding-3-large"));
        assert!(!is_supported_openai_model("whisper-1"));
    }

    #[test]
    fn load_api_key_auth_reads_the_supplied_store() {
        let store = MemoryCredentialStore::default();
        save_provider_api_key(&store, "anthropic", "sk-ant-test").unwrap();

        assert_eq!(
            load_api_key_auth("anthropic", &store).unwrap(),
            "sk-ant-test"
        );
    }

    #[test]
    fn parses_github_copilot_models_from_data_objects_and_deduplicates() {
        let value = serde_json::json!({
            "data": [
                {"id": "gpt-4.1"},
                {"name": "claude-sonnet-4"},
                {"id": "gpt-4.1"}
            ]
        });

        assert_eq!(
            parse_github_copilot_models("github-copilot", &value).unwrap(),
            vec![
                ProviderModel {
                    provider: "github-copilot".into(),
                    model: "claude-sonnet-4".into(),
                    display_name: "claude-sonnet-4".into(),
                    max_output_tokens: None,
                },
                ProviderModel {
                    provider: "github-copilot".into(),
                    model: "gpt-4.1".into(),
                    display_name: "gpt-4.1".into(),
                    max_output_tokens: None,
                },
            ]
        );
    }

    #[tokio::test]
    async fn github_copilot_models_retry_once_after_unauthorized() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let base_url_for_server = base_url.clone();
        tokio::spawn(async move {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 1024];
                let bytes = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..bytes]);
                let is_model_request = request.contains("GET /models");
                let (status, body) = match (index, is_model_request) {
                    (0, true) => ("401 Unauthorized", String::new()),
                    (1, false) => (
                        "200 OK",
                        format!(
                            "{{\"token\":\"second\",\"endpoints\":{{\"api\":\"{base_url_for_server}\"}}}}"
                        ),
                    ),
                    (2, true) => (
                        "200 OK",
                        r#"{"data":[{"id":"gpt-4.1"}]}"#.to_string(),
                    ),
                    _ => ("500 Internal Server Error", String::new()),
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(), body
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let store = MemoryCredentialStore::default();
        save_github_copilot_tokens(
            &store,
            &GitHubCopilotTokens {
                github_access_token: "github".into(),
                github_refresh_token: None,
                github_expires_at_unix: None,
                copilot_token: Some("first".into()),
                copilot_expires_at_unix: Some(i64::MAX),
                copilot_refresh_after_unix: None,
                copilot_token_endpoint: Some(base_url.clone()),
                copilot_chat_endpoint: None,
                copilot_models_endpoint: Some(format!("{base_url}/models")),
            },
        )
        .unwrap();

        assert_eq!(
            fetch_github_copilot_models("github-copilot", &store)
                .await
                .unwrap(),
            vec![ProviderModel {
                provider: "github-copilot".into(),
                model: "gpt-4.1".into(),
                display_name: "gpt-4.1".into(),
                max_output_tokens: None,
            }]
        );
    }

    #[test]
    fn provider_model_cache_replaces_one_provider_and_preserves_max_tokens() {
        let cache_dir = unique_test_cache_dir("replace");
        with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
            replace_cached_provider_models(
                "openai",
                &[ProviderModel {
                    provider: "openai".into(),
                    model: "gpt-5.5".into(),
                    display_name: "gpt-5.5".into(),
                    max_output_tokens: None,
                }],
            )
            .unwrap();
            replace_cached_provider_models(
                "anthropic",
                &[
                    ProviderModel {
                        provider: "anthropic".into(),
                        model: "claude-b".into(),
                        display_name: "Claude B".into(),
                        max_output_tokens: Some(64_000),
                    },
                    ProviderModel {
                        provider: "anthropic".into(),
                        model: "claude-a".into(),
                        display_name: "Claude A".into(),
                        max_output_tokens: Some(32_000),
                    },
                ],
            )
            .unwrap();
            replace_cached_provider_models(
                "anthropic",
                &[ProviderModel {
                    provider: "anthropic".into(),
                    model: "claude-c".into(),
                    display_name: "Claude C".into(),
                    max_output_tokens: Some(16_000),
                }],
            )
            .unwrap();

            assert_eq!(
                cached_provider_models("openai"),
                vec![ProviderModel {
                    provider: "openai".into(),
                    model: "gpt-5.5".into(),
                    display_name: "gpt-5.5".into(),
                    max_output_tokens: None,
                }]
            );
            assert_eq!(
                cached_provider_models("anthropic"),
                vec![ProviderModel {
                    provider: "anthropic".into(),
                    model: "claude-c".into(),
                    display_name: "Claude C".into(),
                    max_output_tokens: Some(16_000),
                }]
            );
        });
        let _ = fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn provider_model_cache_migrates_old_schema() {
        let cache_dir = unique_test_cache_dir("migration");
        fs::create_dir_all(&cache_dir).unwrap();
        let connection = Connection::open(cache_dir.join("provider-models.sqlite3")).unwrap();
        connection
            .execute_batch(
                "create table provider_models (
                    provider text not null,
                    model text not null,
                    display_name text not null,
                    raw_json text,
                    updated_at integer not null,
                    primary key(provider, model)
                );
                create table provider_model_refresh (
                    provider text primary key,
                    updated_at integer not null,
                    error text
                );",
            )
            .unwrap();
        drop(connection);

        with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
            replace_cached_provider_models(
                "anthropic",
                &[ProviderModel {
                    provider: "anthropic".into(),
                    model: "claude-sonnet".into(),
                    display_name: "Claude Sonnet".into(),
                    max_output_tokens: Some(64_000),
                }],
            )
            .unwrap();

            assert_eq!(
                cached_provider_model("anthropic", "claude-sonnet")
                    .and_then(|model| model.max_output_tokens),
                Some(64_000)
            );
        });
        let _ = fs::remove_dir_all(cache_dir);
    }
}
