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
    #[serde(default)]
    pub reasoning_off_behavior: ReasoningOffBehavior,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningOffBehavior {
    #[default]
    Omit,
    EffortNone,
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

    pub fn reasoning_effort(&self, reasoning: ReasoningLevel) -> Option<&str> {
        match (reasoning, self.reasoning_off_behavior) {
            (ReasoningLevel::Off, ReasoningOffBehavior::Omit) => None,
            (ReasoningLevel::Off, ReasoningOffBehavior::EffortNone) => Some("none"),
            _ => reasoning.effort(),
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

pub fn cached_reasoning_effort(
    provider: &str,
    model: &str,
    reasoning: ReasoningLevel,
) -> Option<String> {
    cached_model_metadata(provider, model)
        .map(|metadata| metadata.reasoning_effort(reasoning).map(str::to_string))
        .unwrap_or_else(|| reasoning.effort().map(str::to_string))
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

    // Prefer a live models.dev snapshot for newly seen models. A stale local
    // api.json can predate a provider/model and would otherwise hide pricing
    // until the cache is manually cleared.
    if let Some(response) = fetch_models_dev_api().await {
        write_cached_api(&response);
        if let Some(metadata) = upstream_metadata_from_api(&response, provider, model) {
            write_cached_upstream_model_metadata(provider, model, &metadata);
            return Some(apply_overrides(provider, model, metadata));
        }
    }

    if let Some(metadata) = read_cached_api()
        .as_ref()
        .and_then(|api| upstream_metadata_from_api(api, provider, model))
    {
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
        || metadata.reasoning_off_behavior != ReasoningOffBehavior::Omit
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

/// Bump when the models.dev parser gains fields that older cache rows omit.
/// Rows written with a lower version are treated as misses and re-fetched.
const MODEL_METADATA_CACHE_VERSION: i64 = 2;

fn cached_upstream_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    let upstream_provider = upstream_provider(provider);
    let connection = open_models_dev_cache().ok()?;
    let (contents, cache_version): (String, i64) = connection
        .query_row(
            "select metadata_json, cache_version from model_metadata
             where provider = ?1 and model = ?2",
            params![upstream_provider, model],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    if cache_version < MODEL_METADATA_CACHE_VERSION {
        return None;
    }
    serde_json::from_str(&contents).ok()
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
        "insert into model_metadata (provider, model, metadata_json, updated_at, cache_version)
         values (?1, ?2, ?3, strftime('%s', 'now'), ?4)
         on conflict(provider, model) do update set
           metadata_json = excluded.metadata_json,
           updated_at = excluded.updated_at,
           cache_version = excluded.cache_version",
        params![
            upstream_provider,
            model,
            contents,
            MODEL_METADATA_CACHE_VERSION
        ],
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
            cache_version integer not null default 1,
            primary key (provider, model)
        );",
    )?;
    let _ = connection.execute(
        "alter table model_metadata add column cache_version integer not null default 1",
        [],
    );
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
    let (long_context_threshold, cost_long_context) = long_context_cost_from_api(cost);
    Some(ModelMetadata {
        advertised_context_window: limit
            .and_then(|limit| limit.get("context"))
            .and_then(|value| value.as_u64()),
        effective_context_window: limit
            .and_then(|limit| limit.get("input").or_else(|| limit.get("context")))
            .and_then(|value| value.as_u64()),
        usable_context_window: None,
        long_context_threshold,
        max_output_tokens: limit
            .and_then(|limit| limit.get("output"))
            .and_then(|value| value.as_u64()),
        cost_default: model_cost_from_api(cost),
        cost_long_context,
        supported_reasoning_levels: supported_reasoning_levels(model),
        reasoning_off_behavior: if advertised_none_effort(model) {
            ReasoningOffBehavior::EffortNone
        } else {
            ReasoningOffBehavior::Omit
        },
    })
}

fn advertised_none_effort(model: &Value) -> bool {
    effort_values(model).is_some_and(|values| values.iter().any(|value| value == "none"))
}

fn effort_values(model: &Value) -> Option<&[Value]> {
    model
        .get("reasoning_options")?
        .as_array()?
        .iter()
        .find(|option| option.get("type").and_then(Value::as_str) == Some("effort"))?
        .get("values")?
        .as_array()
        .map(Vec::as_slice)
}

fn supported_reasoning_levels(model: &Value) -> Option<Vec<ReasoningLevel>> {
    let supports_reasoning = model.get("reasoning")?.as_bool()?;
    let reasoning_options = model.get("reasoning_options").and_then(Value::as_array);
    if reasoning_options.is_some_and(Vec::is_empty) {
        return Some(vec![ReasoningLevel::Off]);
    }
    let Some(effort_values) = effort_values(model) else {
        return if supports_reasoning {
            None
        } else {
            Some(vec![ReasoningLevel::Off])
        };
    };

    let mut levels = effort_values
        .iter()
        .filter_map(|value| match value.as_str()? {
            "none" => None,
            "minimal" => Some(ReasoningLevel::Minimal),
            "low" => Some(ReasoningLevel::Low),
            "medium" => Some(ReasoningLevel::Medium),
            "high" => Some(ReasoningLevel::High),
            "xhigh" => Some(ReasoningLevel::Xhigh),
            "max" => Some(ReasoningLevel::Max),
            _ => None,
        })
        .collect::<Vec<_>>();
    if levels.is_empty() && !advertised_none_effort(model) {
        return None;
    }
    levels.push(ReasoningLevel::Off);
    levels.sort_unstable();
    levels.dedup();
    (!levels.is_empty()).then_some(levels)
}

fn model_cost_from_api(cost: Option<&Value>) -> Option<ModelCost> {
    let cost = cost?;
    let model_cost = ModelCost {
        input_micros_per_m: cost.get("input").and_then(cost_micros_per_million),
        output_micros_per_m: cost.get("output").and_then(cost_micros_per_million),
        cache_read_micros_per_m: cost.get("cache_read").and_then(cost_micros_per_million),
        cache_write_micros_per_m: cost.get("cache_write").and_then(cost_micros_per_million),
    };
    model_cost_has_rates(&model_cost).then_some(model_cost)
}

fn long_context_cost_from_api(cost: Option<&Value>) -> (Option<u64>, Option<ModelCost>) {
    let Some(cost) = cost else {
        return (None, None);
    };

    if let Some(tiers) = cost.get("tiers").and_then(Value::as_array) {
        for tier in tiers {
            let Some(threshold) = tier
                .get("tier")
                .and_then(|tier| tier.get("size"))
                .and_then(Value::as_u64)
            else {
                continue;
            };
            let Some(model_cost) = model_cost_from_api(Some(tier)) else {
                continue;
            };
            return (Some(threshold), Some(model_cost));
        }
    }

    let Some(object) = cost.as_object() else {
        return (None, None);
    };
    for (key, value) in object {
        let Some(threshold) = context_over_threshold(key) else {
            continue;
        };
        let Some(model_cost) = model_cost_from_api(Some(value)) else {
            continue;
        };
        return (Some(threshold), Some(model_cost));
    }

    (None, None)
}

fn context_over_threshold(key: &str) -> Option<u64> {
    let rest = key.strip_prefix("context_over_")?;
    let (amount, unit) = rest.split_at(rest.find(|c: char| !c.is_ascii_digit())?);
    let amount = amount.parse::<u64>().ok()?;
    let multiplier = match unit {
        "k" | "K" => 1_000,
        "m" | "M" => 1_000_000,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

fn model_cost_has_rates(cost: &ModelCost) -> bool {
    cost.input_micros_per_m.is_some()
        || cost.output_micros_per_m.is_some()
        || cost.cache_read_micros_per_m.is_some()
        || cost.cache_write_micros_per_m.is_some()
}
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
    levels.push(ReasoningLevel::Off);
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
    use pretty_assertions::assert_eq;
    use serde_json::json;

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
            metadata.reasoning_off_behavior,
            ReasoningOffBehavior::EffortNone
        );
        assert_eq!(metadata.reasoning_effort(ReasoningLevel::Off), Some("none"));
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
    fn effort_options_without_none_still_support_off_by_omission() {
        let api = serde_json::json!({
            "openai": {
                "models": {
                    "gpt-test": {
                        "reasoning": true,
                        "reasoning_options": [{
                            "type": "effort",
                            "values": ["low", "medium", "high", "xhigh"]
                        }]
                    }
                }
            }
        });

        let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

        assert_eq!(metadata.reasoning_off_behavior, ReasoningOffBehavior::Omit);
        assert_eq!(metadata.reasoning_effort(ReasoningLevel::Off), None);
        assert_eq!(
            metadata.supported_reasoning_levels,
            Some(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
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

    #[test]
    fn models_dev_parses_long_context_cost_tiers() {
        let api = json!({
            "xai": {
                "models": {
                    "grok-4.5": {
                        "limit": { "context": 500000, "output": 500000 },
                        "cost": {
                            "input": 2.0,
                            "output": 6.0,
                            "cache_read": 0.5,
                            "tiers": [{
                                "input": 4.0,
                                "output": 12.0,
                                "cache_read": 1.0,
                                "tier": { "type": "context", "size": 200000 }
                            }],
                            "context_over_200k": {
                                "input": 4.0,
                                "output": 12.0,
                                "cache_read": 1.0
                            }
                        }
                    }
                }
            }
        });

        let metadata = model_metadata_from_api(&api, "xai", "grok-4.5").unwrap();

        assert_eq!(
            metadata,
            ModelMetadata {
                advertised_context_window: Some(500_000),
                effective_context_window: Some(500_000),
                usable_context_window: None,
                long_context_threshold: Some(200_000),
                max_output_tokens: Some(500_000),
                cost_default: Some(ModelCost {
                    input_micros_per_m: Some(2_000_000),
                    output_micros_per_m: Some(6_000_000),
                    cache_read_micros_per_m: Some(500_000),
                    cache_write_micros_per_m: None,
                }),
                cost_long_context: Some(ModelCost {
                    input_micros_per_m: Some(4_000_000),
                    output_micros_per_m: Some(12_000_000),
                    cache_read_micros_per_m: Some(1_000_000),
                    cache_write_micros_per_m: None,
                }),
            }
        );
        assert_eq!(
            metadata
                .cost_for_input_tokens(200_001)
                .unwrap()
                .input_micros_per_m,
            Some(4_000_000)
        );
        assert_eq!(
            metadata
                .cost_for_input_tokens(200_000)
                .unwrap()
                .input_micros_per_m,
            Some(2_000_000)
        );
    }
}
