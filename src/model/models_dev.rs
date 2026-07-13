use std::{fs, path::PathBuf, sync::OnceLock, time::Duration};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::reasoning::ReasoningLevel;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModelMetadata {
    pub advertised_context_window: Option<u64>,
    pub effective_context_window: Option<u64>,
    pub usable_context_window: Option<u64>,
    pub long_context_threshold: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub cost_default: Option<ModelCost>,
    pub cost_long_context: Option<ModelCost>,
    pub supported_reasoning_levels: Option<Vec<ReasoningLevel>>,
}

impl ModelMetadata {
    pub fn display_context_window(&self) -> Option<u64> {
        self.usable_context_window
            .or(self.effective_context_window)
            .or(self.advertised_context_window)
    }

    pub fn cost_for_input_tokens(&self, input_tokens: u64) -> Option<ModelCost> {
        if self
            .long_context_threshold
            .is_some_and(|threshold| input_tokens > threshold)
        {
            self.cost_long_context.or(self.cost_default)
        } else {
            self.cost_default
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModelCost {
    pub input_micros_per_m: Option<u64>,
    pub output_micros_per_m: Option<u64>,
    pub cache_read_micros_per_m: Option<u64>,
    pub cache_write_micros_per_m: Option<u64>,
}

pub fn cached_reasoning_levels(provider: &str, model: &str) -> Option<Vec<ReasoningLevel>> {
    cached_model_metadata(provider, model)?.supported_reasoning_levels
}

pub fn cached_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    cached_upstream_model_metadata(provider, model)
        .map(|metadata| apply_overrides(provider, model, metadata))
        .or_else(|| override_metadata(provider, model))
}

pub async fn fetch_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    if let Some(metadata) = cached_upstream_model_metadata(provider, model) {
        return Some(apply_overrides(provider, model, metadata));
    }

    if let Some(metadata) = read_cached_api()
        .as_ref()
        .and_then(|api| upstream_metadata_from_api(api, provider, model))
    {
        write_cached_upstream_model_metadata(provider, model, &metadata);
        return Some(apply_overrides(provider, model, metadata));
    }

    let Some(response) = fetch_models_dev_api().await else {
        return override_metadata(provider, model);
    };
    write_cached_api(&response);
    if let Some(metadata) = upstream_metadata_from_api(&response, provider, model) {
        write_cached_upstream_model_metadata(provider, model, &metadata);
        return Some(apply_overrides(provider, model, metadata));
    }
    override_metadata(provider, model)
}

fn upstream_metadata_from_api(api: &Value, provider: &str, model: &str) -> Option<ModelMetadata> {
    model_metadata_from_api(api, upstream_provider(provider), model)
}

fn apply_overrides(provider: &str, model: &str, metadata: ModelMetadata) -> ModelMetadata {
    let metadata = apply_builtin_overrides(provider, model, metadata);
    apply_local_overrides(provider, model, metadata)
}

fn override_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    let metadata = apply_overrides(provider, model, ModelMetadata::default());
    metadata_has_values(&metadata).then_some(metadata)
}

fn metadata_has_values(metadata: &ModelMetadata) -> bool {
    metadata.advertised_context_window.is_some()
        || metadata.effective_context_window.is_some()
        || metadata.usable_context_window.is_some()
        || metadata.long_context_threshold.is_some()
        || metadata.max_output_tokens.is_some()
        || metadata.cost_default.is_some()
        || metadata.cost_long_context.is_some()
        || metadata.supported_reasoning_levels.is_some()
}

async fn fetch_models_dev_api() -> Option<Value> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?
        .get("https://models.dev/api.json")
        .header("User-Agent", concat!("rho/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()
}

fn read_cached_api() -> Option<Value> {
    let contents = fs::read_to_string(models_dev_cache_path()).ok()?;
    serde_json::from_str(&contents).ok()
}

fn write_cached_api(value: &Value) {
    let path = models_dev_cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(contents) = serde_json::to_string(value) {
        let _ = fs::write(path, contents);
    }
}

fn cached_upstream_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    let upstream_provider = upstream_provider(provider);
    let connection = open_models_dev_cache().ok()?;
    let contents: String = connection
        .query_row(
            "select metadata_json from model_metadata where provider = ?1 and model = ?2",
            params![upstream_provider, model],
            |row| row.get(0),
        )
        .ok()?;
    let cached_value: Value = serde_json::from_str(&contents).ok()?;
    let reasoning_levels_cached = cached_value.get("supported_reasoning_levels").is_some();
    let cached: ModelMetadata = serde_json::from_value(cached_value).ok()?;
    if reasoning_levels_cached {
        return Some(cached);
    }

    let refreshed = read_cached_api()
        .as_ref()
        .and_then(|api| model_metadata_from_api(api, upstream_provider, model));
    if let Some(refreshed) = refreshed {
        write_cached_upstream_model_metadata(provider, model, &refreshed);
        Some(refreshed)
    } else {
        Some(cached)
    }
}

fn write_cached_upstream_model_metadata(provider: &str, model: &str, metadata: &ModelMetadata) {
    let upstream_provider = upstream_provider(provider);
    let Ok(connection) = open_models_dev_cache() else {
        return;
    };
    let Ok(contents) = serde_json::to_string(metadata) else {
        return;
    };
    let _ = connection.execute(
        "insert into model_metadata (provider, model, metadata_json, updated_at)
         values (?1, ?2, ?3, strftime('%s', 'now'))
         on conflict(provider, model) do update set
           metadata_json = excluded.metadata_json,
           updated_at = excluded.updated_at",
        params![upstream_provider, model, contents],
    );
}

fn open_models_dev_cache() -> rusqlite::Result<Connection> {
    let path = models_dev_sqlite_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "create table if not exists model_metadata (
            provider text not null,
            model text not null,
            metadata_json text not null,
            updated_at integer not null,
            primary key (provider, model)
        );",
    )?;
    Ok(connection)
}

fn upstream_provider(provider: &str) -> &str {
    crate::provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.metadata_upstream)
        .unwrap_or(provider)
}

fn models_dev_sqlite_path() -> PathBuf {
    cache_dir().join("models.dev/models-dev-metadata.sqlite3")
}

fn models_dev_cache_path() -> PathBuf {
    cache_dir().join("models.dev/api.json")
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
        if let Some(path) = crate::paths::home_dir() {
            return path.join("Library").join("Caches").join("rho");
        }
    }
    if let Some(path) = crate::paths::home_dir() {
        return path.join(".cache").join("rho");
    }
    std::env::temp_dir().join("rho-cache")
}

fn model_metadata_from_api(api: &Value, provider: &str, model: &str) -> Option<ModelMetadata> {
    let model = api.get(provider)?.get("models")?.get(model).or_else(|| {
        api.get(provider)?
            .get("models")?
            .get(model.strip_prefix("openai/")?)
    })?;
    let limit = model.get("limit");
    let cost = model.get("cost");
    Some(ModelMetadata {
        advertised_context_window: limit
            .and_then(|limit| limit.get("context"))
            .and_then(|value| value.as_u64()),
        effective_context_window: limit
            .and_then(|limit| limit.get("input").or_else(|| limit.get("context")))
            .and_then(|value| value.as_u64()),
        usable_context_window: None,
        long_context_threshold: None,
        max_output_tokens: limit
            .and_then(|limit| limit.get("output"))
            .and_then(|value| value.as_u64()),
        cost_default: Some(ModelCost {
            input_micros_per_m: cost
                .and_then(|cost| cost.get("input"))
                .and_then(cost_micros_per_million),
            output_micros_per_m: cost
                .and_then(|cost| cost.get("output"))
                .and_then(cost_micros_per_million),
            cache_read_micros_per_m: cost
                .and_then(|cost| cost.get("cache_read"))
                .and_then(cost_micros_per_million),
            cache_write_micros_per_m: cost
                .and_then(|cost| cost.get("cache_write"))
                .and_then(cost_micros_per_million),
        }),
        cost_long_context: None,
        supported_reasoning_levels: supported_reasoning_levels(model),
    })
}

fn supported_reasoning_levels(model: &Value) -> Option<Vec<ReasoningLevel>> {
    let supports_reasoning = model.get("reasoning")?.as_bool()?;
    let reasoning_options = model.get("reasoning_options").and_then(Value::as_array);
    if reasoning_options.is_some_and(Vec::is_empty) {
        return Some(vec![ReasoningLevel::Off]);
    }
    let effort_values = reasoning_options.and_then(|options| {
        options.iter().find_map(|option| {
            (option.get("type").and_then(Value::as_str) == Some("effort"))
                .then(|| option.get("values").and_then(Value::as_array))
                .flatten()
        })
    });
    let Some(effort_values) = effort_values else {
        return if supports_reasoning {
            None
        } else {
            Some(vec![ReasoningLevel::Off])
        };
    };

    let mut levels = effort_values
        .iter()
        .filter_map(|value| match value.as_str()? {
            "none" => Some(ReasoningLevel::Off),
            "minimal" => Some(ReasoningLevel::Minimal),
            "low" => Some(ReasoningLevel::Low),
            "medium" => Some(ReasoningLevel::Medium),
            "high" => Some(ReasoningLevel::High),
            "xhigh" => Some(ReasoningLevel::Xhigh),
            "max" => Some(ReasoningLevel::Max),
            _ => None,
        })
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    (!levels.is_empty()).then_some(levels)
}

const BUILTIN_MODEL_OVERRIDES_TOML: &str = include_str!("model_overrides.toml");

fn apply_builtin_overrides(provider: &str, model: &str, metadata: ModelMetadata) -> ModelMetadata {
    static OVERRIDES: OnceLock<toml::Value> = OnceLock::new();
    let overrides = OVERRIDES.get_or_init(|| {
        BUILTIN_MODEL_OVERRIDES_TOML
            .parse()
            .expect("built-in model overrides must be valid TOML")
    });
    let key = format!("{provider}/{model}");
    let Some(table) = overrides
        .get("models")
        .and_then(|models| models.get(&key))
        .and_then(toml::Value::as_table)
    else {
        return metadata;
    };

    merge_toml_override(metadata, table)
}

fn apply_local_overrides(provider: &str, model: &str, metadata: ModelMetadata) -> ModelMetadata {
    let Some(path) = local_overrides_path() else {
        return metadata;
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return metadata;
    };
    let Ok(value) = contents.parse::<toml::Value>() else {
        return metadata;
    };
    let key = format!("{provider}/{model}");
    let Some(table) = value
        .get("models")
        .and_then(|models| models.get(&key))
        .and_then(|value| value.as_table())
    else {
        return metadata;
    };

    merge_toml_override(metadata, table)
}

fn local_overrides_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RHO_MODELS_PATH") {
        return Some(path.into());
    }
    Some(crate::paths::rho_dir().ok()?.join("models.toml"))
}

fn merge_toml_override(
    mut metadata: ModelMetadata,
    table: &toml::map::Map<String, toml::Value>,
) -> ModelMetadata {
    metadata.advertised_context_window =
        toml_u64(table, "advertised_context_window").or(metadata.advertised_context_window);
    metadata.effective_context_window =
        toml_u64(table, "effective_context_window").or(metadata.effective_context_window);
    metadata.usable_context_window =
        toml_u64(table, "usable_context_window").or(metadata.usable_context_window);
    metadata.long_context_threshold =
        toml_u64(table, "long_context_threshold").or(metadata.long_context_threshold);
    metadata.max_output_tokens =
        toml_u64(table, "max_output_tokens").or(metadata.max_output_tokens);
    metadata.cost_default = toml_cost(table, "cost_default").or(metadata.cost_default);
    metadata.cost_long_context =
        toml_cost(table, "cost_long_context").or(metadata.cost_long_context);
    if let Some(levels) = toml_reasoning_levels(table, "supported_reasoning_levels") {
        metadata.supported_reasoning_levels = Some(levels);
    }
    metadata
}

fn toml_reasoning_levels(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
) -> Option<Vec<ReasoningLevel>> {
    let mut levels = table
        .get(key)?
        .as_array()?
        .iter()
        .filter_map(toml::Value::as_str)
        .filter_map(|value| value.parse().ok())
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    Some(levels)
}

fn toml_u64(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<u64> {
    table
        .get(key)
        .and_then(|value| value.as_integer())
        .and_then(|value| u64::try_from(value).ok())
}

fn toml_cost(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<ModelCost> {
    let table = table.get(key)?.as_table()?;
    Some(ModelCost {
        input_micros_per_m: toml_cost_value(table, "input"),
        output_micros_per_m: toml_cost_value(table, "output"),
        cache_read_micros_per_m: toml_cost_value(table, "cache_read"),
        cache_write_micros_per_m: toml_cost_value(table, "cache_write"),
    })
}

fn toml_cost_value(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<u64> {
    let dollars = table.get(key).and_then(|value| {
        value
            .as_float()
            .or_else(|| value.as_integer().map(|v| v as f64))
    })?;
    dollars
        .is_finite()
        .then(|| (dollars.max(0.0) * 1_000_000.0).round() as u64)
}

fn cost_micros_per_million(value: &Value) -> Option<u64> {
    let dollars = value.as_f64().or_else(|| {
        value
            .as_str()?
            .trim_start_matches('$')
            .replace(',', "")
            .parse()
            .ok()
    })?;
    dollars
        .is_finite()
        .then(|| (dollars.max(0.0) * 1_000_000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reasoning_effort_options() {
        let api = serde_json::json!({
            "openai": {
                "models": {
                    "gpt-test": {
                        "reasoning": true,
                        "reasoning_options": [{
                            "type": "effort",
                            "values": ["none", "low", "high", "xhigh"]
                        }]
                    }
                }
            }
        });

        let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

        assert_eq!(
            metadata.supported_reasoning_levels,
            Some(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh,
            ])
        );
    }

    #[test]
    fn unknown_effort_values_do_not_restrict_reasoning() {
        let api = serde_json::json!({
            "openai": {
                "models": {
                    "gpt-test": {
                        "reasoning": true,
                        "reasoning_options": [{"type": "effort", "values": ["default"]}]
                    }
                }
            }
        });

        let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

        assert_eq!(metadata.supported_reasoning_levels, None);
    }

    #[test]
    fn models_without_effort_choices_only_expose_off() {
        let api = serde_json::json!({
            "openai": {
                "models": {
                    "gpt-test": {"reasoning": true, "reasoning_options": []}
                }
            }
        });

        let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

        assert_eq!(
            metadata.supported_reasoning_levels,
            Some(vec![ReasoningLevel::Off])
        );
    }

    #[test]
    fn leaves_unknown_reasoning_option_schemas_unrestricted() {
        let api = serde_json::json!({
            "anthropic": {
                "models": {
                    "claude-test": {
                        "reasoning": true,
                        "reasoning_options": [{"type": "budget_tokens", "min": 1024}]
                    }
                }
            }
        });

        let metadata = model_metadata_from_api(&api, "anthropic", "claude-test").unwrap();

        assert_eq!(metadata.supported_reasoning_levels, None);
    }

    #[test]
    fn non_reasoning_models_only_support_off() {
        let api = serde_json::json!({
            "openai": {"models": {"gpt-test": {"reasoning": false}}}
        });

        let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

        assert_eq!(
            metadata.supported_reasoning_levels,
            Some(vec![ReasoningLevel::Off])
        );
    }

    #[test]
    fn builtin_gpt_56_codex_overrides_use_temporary_context_limit() {
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let metadata = apply_builtin_overrides("openai-codex", model, ModelMetadata::default());

            assert_eq!(metadata.effective_context_window, Some(272_000));
            assert_eq!(metadata.usable_context_window, Some(272_000));
            assert_eq!(metadata.display_context_window(), Some(272_000));
        }
    }

    #[test]
    fn builtin_gpt_55_overrides_use_safer_effective_windows() {
        let upstream = ModelMetadata {
            advertised_context_window: Some(1_050_000),
            effective_context_window: Some(922_000),
            max_output_tokens: Some(128_000),
            cost_default: Some(ModelCost {
                input_micros_per_m: Some(5_000_000),
                output_micros_per_m: Some(30_000_000),
                cache_read_micros_per_m: Some(500_000),
                cache_write_micros_per_m: None,
            }),
            ..ModelMetadata::default()
        };
        let openai = apply_builtin_overrides("openai", "gpt-5.5", upstream.clone());
        let codex = apply_builtin_overrides("openai-codex", "gpt-5.5", upstream);

        assert_eq!(openai.display_context_window(), Some(272_000));
        assert_eq!(openai.effective_context_window, Some(922_000));
        assert_eq!(codex.display_context_window(), Some(272_000));
        assert_eq!(codex.effective_context_window, Some(400_000));
        assert_eq!(codex.advertised_context_window, Some(1_050_000));
        assert_eq!(codex.long_context_threshold, Some(272_000));
        assert_eq!(codex.max_output_tokens, Some(128_000));
        assert_eq!(
            codex.cost_default.unwrap().input_micros_per_m,
            Some(5_000_000)
        );
    }
}
