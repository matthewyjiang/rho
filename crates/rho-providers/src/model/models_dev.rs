use std::{collections::HashSet, fs, path::PathBuf, sync::OnceLock, time::Duration};

#[cfg(test)]
use std::cell::RefCell;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::ReasoningCapabilities, provider::CatalogReasoningPolicy, reasoning::ReasoningLevel,
};

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
    /// Whether the resolved capability itself is exact. This is intentionally
    /// separate from metadata completeness because some provider policies
    /// resolve complete catalog data to `Unknown`.
    #[serde(default)]
    pub reasoning_capabilities_known: bool,
    /// True once the catalog reasoning fields have been fully parsed and the
    /// provider policy has been applied. A complete row may intentionally have
    /// unknown capabilities.
    #[serde(default)]
    pub reasoning_metadata_complete: bool,
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

    pub fn reasoning_capabilities(&self) -> ReasoningCapabilities {
        ReasoningCapabilities::from_metadata(
            self.supported_reasoning_levels.clone(),
            self.reasoning_capabilities_known,
        )
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

pub fn current_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    current_cached_upstream_model_metadata(provider, model)
        .map(|metadata| apply_overrides(provider, model, metadata))
        .or_else(|| override_metadata(provider, model))
}

pub fn current_reasoning_capabilities(provider: &str, model: &str) -> ReasoningCapabilities {
    if provider_reasoning_is_not_configurable(provider) {
        return ReasoningCapabilities::NotConfigurable;
    }
    current_model_metadata(provider, model)
        .map(|metadata| metadata.reasoning_capabilities())
        .unwrap_or_default()
}

pub fn cached_reasoning_capabilities(provider: &str, model: &str) -> ReasoningCapabilities {
    if provider_reasoning_is_not_configurable(provider) {
        return ReasoningCapabilities::NotConfigurable;
    }
    cached_model_metadata(provider, model)
        .map(|metadata| metadata.reasoning_capabilities())
        .unwrap_or_default()
}

fn provider_reasoning_is_not_configurable(provider: &str) -> bool {
    crate::provider::provider_descriptor(provider).is_some_and(|descriptor| {
        descriptor.catalog_reasoning == CatalogReasoningPolicy::NotConfigurable
    })
}

pub fn model_metadata_needs_refresh(provider: &str, model: &str) -> bool {
    if provider_reasoning_is_not_configurable(provider) {
        return false;
    }
    current_cached_upstream_model_metadata(provider, model)
        .map(|metadata| apply_overrides(provider, model, metadata))
        .or_else(|| override_metadata(provider, model))
        .is_none_or(|metadata| !metadata.reasoning_metadata_complete)
}

pub fn cached_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    cached_upstream_model_metadata(provider, model)
        .map(|metadata| apply_overrides(provider, model, metadata))
        .or_else(|| override_metadata(provider, model))
}

pub async fn fetch_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    if let Some(metadata) = current_cached_upstream_model_metadata(provider, model) {
        return Some(apply_overrides(provider, model, metadata));
    }

    // Prefer a live models.dev snapshot for newly seen models. A stale local
    // api.json can predate a provider/model and would otherwise hide pricing
    // until the cache is manually cleared.
    if let Some(response) = fetch_models_dev_api().await {
        write_cached_api(&response);
        if let Some(metadata) = upstream_metadata_from_api(&response, provider, model) {
            if metadata.reasoning_metadata_complete {
                write_cached_upstream_model_metadata(provider, model, &metadata);
            }
            return Some(apply_overrides(provider, model, metadata));
        }
    }

    override_metadata(provider, model)
}

fn upstream_metadata_from_api(api: &Value, provider: &str, model: &str) -> Option<ModelMetadata> {
    let descriptor = crate::provider::provider_descriptor(provider)?;
    model_metadata_from_api_with_policy(
        api,
        descriptor.metadata_upstream_for_model(model),
        descriptor.metadata_model(model),
        descriptor.catalog_reasoning,
    )
}

fn apply_overrides(provider: &str, model: &str, metadata: ModelMetadata) -> ModelMetadata {
    let metadata = apply_builtin_overrides(provider, model, metadata);
    let metadata = apply_provider_capabilities(provider, model, metadata);
    apply_local_overrides(provider, model, metadata)
}

fn apply_provider_capabilities(
    provider: &str,
    model: &str,
    mut metadata: ModelMetadata,
) -> ModelMetadata {
    let provider_model = super::provider_models::cached_provider_model(provider, model);
    let context_window = provider_model
        .as_ref()
        .and_then(|model| model.context_window)
        .or_else(|| {
            crate::provider::provider_descriptor(provider)
                .and_then(|descriptor| descriptor.effective_context_fallback(model))
        });
    if let Some(context_window) = context_window {
        metadata.effective_context_window = Some(context_window);
    }
    if let Some(provider_model) = provider_model.filter(|_| {
        !super::provider_models::provider_model_capabilities_need_refresh(provider, model)
    }) {
        match provider_model.reasoning_capabilities {
            ReasoningCapabilities::Unknown => {}
            ReasoningCapabilities::NotConfigurable => {
                metadata.supported_reasoning_levels = None;
                metadata.reasoning_capabilities_known = true;
                metadata.reasoning_metadata_complete = true;
            }
            ReasoningCapabilities::Levels(levels) => {
                metadata.supported_reasoning_levels = Some(levels.into_levels());
                metadata.reasoning_capabilities_known = true;
                metadata.reasoning_metadata_complete = true;
            }
        }
    }
    metadata
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
        || metadata.reasoning_capabilities_known
        || metadata.reasoning_metadata_complete
        || metadata.reasoning_off_behavior != ReasoningOffBehavior::Omit
}

pub(crate) async fn fetch_deprecated_provider_models(provider: &str) -> Option<HashSet<String>> {
    let response = fetch_models_dev_api().await?;
    write_cached_api(&response);
    Some(deprecated_provider_models_from_api(&response, provider))
}

fn deprecated_provider_models_from_api(api: &Value, provider: &str) -> HashSet<String> {
    api.get(provider)
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, model)| model.get("status").and_then(Value::as_str) == Some("deprecated"))
        .map(|(id, _)| id.clone())
        .collect()
}

async fn fetch_models_dev_api() -> Option<Value> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?
        .get("https://models.dev/api.json")
        .header("User-Agent", crate::rho_user_agent())
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()
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
/// Older or incomplete rows remain available as stale offline fallback, while
/// explicit fetch paths rehydrate and write them from a catalog snapshot.
const MODEL_METADATA_CACHE_VERSION: i64 = 5;

fn cached_upstream_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    cached_upstream_model_metadata_with_freshness(provider, model, CacheFreshness::AllowStale)
}

fn current_cached_upstream_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    cached_upstream_model_metadata_with_freshness(provider, model, CacheFreshness::CurrentOnly)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheFreshness {
    CurrentOnly,
    AllowStale,
}

fn cached_upstream_model_metadata_with_freshness(
    provider: &str,
    model: &str,
    freshness: CacheFreshness,
) -> Option<ModelMetadata> {
    let cache_provider = provider;
    let cache_model = model;
    let connection = open_models_dev_cache().ok()?;
    let (contents, cache_version): (String, i64) = connection
        .query_row(
            "select metadata_json, cache_version from model_metadata
             where provider = ?1 and model = ?2",
            params![cache_provider, cache_model],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    let cached: ModelMetadata = serde_json::from_str(&contents).ok()?;
    if !should_rehydrate_cached_metadata(cache_version, &cached) {
        return Some(cached);
    }

    // Reads are side-effect free. Explicit fetch paths own catalog rehydration
    // and only advance the cache after parsing a complete snapshot.
    (freshness == CacheFreshness::AllowStale).then_some(cached)
}

fn should_rehydrate_cached_metadata(cache_version: i64, cached: &ModelMetadata) -> bool {
    cache_version < MODEL_METADATA_CACHE_VERSION || !cached.reasoning_metadata_complete
}

fn write_cached_upstream_model_metadata(provider: &str, model: &str, metadata: &ModelMetadata) {
    let cache_provider = provider;
    let cache_model = model;
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
            cache_provider,
            cache_model,
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

fn models_dev_sqlite_path() -> PathBuf {
    cache_dir().join("models.dev/models-dev-metadata.sqlite3")
}

fn models_dev_cache_path() -> PathBuf {
    cache_dir().join("models.dev/api.json")
}

fn cache_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_CACHE_DIR.with(|path| path.borrow().clone()) {
        return path;
    }
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

#[cfg(test)]
thread_local! {
    static TEST_CACHE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_models_dev_cache_dir_for_tests<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    with_models_dev_cache_dir(path, f)
}

#[cfg(test)]
fn with_models_dev_cache_dir<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    TEST_CACHE_DIR.with(|cache_dir| {
        let previous = cache_dir.replace(Some(path));
        let result = f();
        cache_dir.replace(previous);
        result
    })
}

#[cfg(test)]
fn model_metadata_from_api(api: &Value, provider: &str, model: &str) -> Option<ModelMetadata> {
    let policy = crate::provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.catalog_reasoning)
        .unwrap_or(CatalogReasoningPolicy::ExactAdvertised);
    model_metadata_from_api_with_policy(api, provider, model, policy)
}

fn model_metadata_from_api_with_policy(
    api: &Value,
    provider: &str,
    model: &str,
    reasoning_policy: CatalogReasoningPolicy,
) -> Option<ModelMetadata> {
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
        supported_reasoning_levels: supported_reasoning_levels(model, reasoning_policy),
        reasoning_off_behavior: if advertised_none_effort(model) {
            ReasoningOffBehavior::EffortNone
        } else {
            ReasoningOffBehavior::Omit
        },
        reasoning_capabilities_known: reasoning_capabilities_known(model, reasoning_policy),
        reasoning_metadata_complete: reasoning_metadata_complete(model, reasoning_policy),
    })
}

fn reasoning_metadata_complete(model: &Value, policy: CatalogReasoningPolicy) -> bool {
    if matches!(
        policy,
        CatalogReasoningPolicy::Unknown | CatalogReasoningPolicy::NotConfigurable
    ) {
        return true;
    }
    reasoning_capabilities_known(model, policy)
}

fn reasoning_capabilities_known(model: &Value, policy: CatalogReasoningPolicy) -> bool {
    if policy == CatalogReasoningPolicy::Unknown {
        // Anthropic's adaptive, mandatory, disabled, and budget-token protocols
        // cannot be represented faithfully as one generic exact level set yet.
        return false;
    }
    if policy == CatalogReasoningPolicy::NotConfigurable {
        return true;
    }
    let Some(supports_reasoning) = model.get("reasoning").and_then(Value::as_bool) else {
        // Missing capability signal: keep the row incomplete so a fresher
        // models.dev snapshot can still be fetched.
        return false;
    };
    if !supports_reasoning {
        return true;
    }
    let Some(options) = model.get("reasoning_options").and_then(Value::as_array) else {
        return false;
    };
    if options.is_empty() {
        return true;
    }
    effort_values(model)
        .is_some_and(|values| !values.is_empty() && values.iter().all(is_recognized_effort_value))
}

fn is_recognized_effort_value(value: &Value) -> bool {
    matches!(
        value.as_str(),
        Some("none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max")
    )
}

fn advertised_toggle(model: &Value) -> bool {
    model
        .get("reasoning_options")
        .and_then(Value::as_array)
        .is_some_and(|options| {
            options
                .iter()
                .any(|option| option.get("type").and_then(Value::as_str) == Some("toggle"))
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

fn supported_reasoning_levels(
    model: &Value,
    policy: CatalogReasoningPolicy,
) -> Option<Vec<ReasoningLevel>> {
    if matches!(
        policy,
        CatalogReasoningPolicy::Unknown | CatalogReasoningPolicy::NotConfigurable
    ) {
        return None;
    }
    let supports_reasoning = model.get("reasoning")?.as_bool()?;
    if !supports_reasoning {
        return None;
    }
    let reasoning_options = model.get("reasoning_options").and_then(Value::as_array);
    if reasoning_options.is_some_and(Vec::is_empty) {
        return None;
    }
    let effort_values = effort_values(model)?;
    if effort_values.is_empty() || !effort_values.iter().all(is_recognized_effort_value) {
        return None;
    }

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
    if levels.is_empty() && !advertised_none_effort(model) {
        return None;
    }
    if (policy == CatalogReasoningPolicy::OffAsNone
        || (policy == CatalogReasoningPolicy::OffByAdvertisedToggle && advertised_toggle(model)))
        && !levels.contains(&ReasoningLevel::Off)
    {
        levels.push(ReasoningLevel::Off);
    }
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
        metadata.reasoning_capabilities_known = true;
        metadata.reasoning_metadata_complete = true;
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
#[path = "models_dev_tests.rs"]
mod tests;
