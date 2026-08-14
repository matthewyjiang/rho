use std::{cell::RefCell, collections::HashSet, fs, path::PathBuf, time::Duration};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::ReasoningCapabilities, provider::CatalogReasoningPolicy, reasoning::ReasoningLevel,
};

#[path = "models_dev_hydrate.rs"]
mod hydrate;
#[path = "models_dev_overrides.rs"]
mod overrides;
#[path = "models_dev_sdk.rs"]
mod sdk;
pub use hydrate::{ensure_models_dev_catalog, prefetch_model_metadata};
use sdk::resolved_sdk_package;
pub use sdk::CatalogSdkAdapter;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModelMetadata {
    /// Catalog name for people, such as `GPT-5.6 Sol`. Absent when the catalog
    /// has no name for the model; callers then show the model id alone rather
    /// than inventing a name from it.
    #[serde(default)]
    pub display_name: Option<String>,
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
    /// Resolved models.dev AI SDK package (`npm` or `provider.npm`).
    #[serde(default)]
    pub sdk_package: Option<String>,
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

    pub fn reasoning_effort(&self, reasoning: ReasoningLevel) -> Option<&'static str> {
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
    if let Some(capabilities) = provider_fixed_reasoning_capabilities(provider) {
        return capabilities;
    }
    current_model_metadata(provider, model)
        .map(|metadata| metadata.reasoning_capabilities())
        .unwrap_or_default()
}

pub fn cached_reasoning_capabilities(provider: &str, model: &str) -> ReasoningCapabilities {
    if let Some(capabilities) = provider_fixed_reasoning_capabilities(provider) {
        return capabilities;
    }
    cached_model_metadata(provider, model)
        .map(|metadata| metadata.reasoning_capabilities())
        .unwrap_or_default()
}

/// Prefer a current row with known reasoning; fall back to a stale-but-known cache row.
///
/// Wire encoding and UI pickers use this so a previously fetched models.dev
/// entry still constrains levels when the current row is missing or incomplete.
/// Preference is reasoning-known-ness, not freshness of other metadata fields.
pub fn known_reasoning_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    let current = current_model_metadata(provider, model);
    if current
        .as_ref()
        .is_some_and(|metadata| metadata.reasoning_capabilities().is_known())
    {
        return current;
    }
    let cached = cached_model_metadata(provider, model);
    if cached
        .as_ref()
        .is_some_and(|metadata| metadata.reasoning_capabilities().is_known())
    {
        return cached;
    }
    current.or(cached)
}

/// Prefer current-known capabilities; fall back to a stale-but-known cache row.
///
/// UI surfaces use this so a previously fetched catalog entry still constrains
/// pickers when the current row is missing or incomplete.
pub fn known_reasoning_capabilities(provider: &str, model: &str) -> ReasoningCapabilities {
    if let Some(capabilities) = provider_fixed_reasoning_capabilities(provider) {
        return capabilities;
    }
    known_reasoning_metadata(provider, model)
        .map(|metadata| metadata.reasoning_capabilities())
        .unwrap_or_default()
}

fn provider_fixed_reasoning_capabilities(provider: &str) -> Option<ReasoningCapabilities> {
    let policy = crate::provider::provider_descriptor(provider)?.catalog_reasoning;
    match policy {
        CatalogReasoningPolicy::NotConfigurable => Some(ReasoningCapabilities::NotConfigurable),
        CatalogReasoningPolicy::OffOrMax => Some(ReasoningCapabilities::Levels(
            crate::model::ReasoningLevelSet::new(vec![ReasoningLevel::Off, ReasoningLevel::Max]),
        )),
        CatalogReasoningPolicy::Unknown
        | CatalogReasoningPolicy::ExactAdvertised
        | CatalogReasoningPolicy::OffByAdvertisedToggle
        | CatalogReasoningPolicy::OffAsNone => None,
    }
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

    // One full catalog hydrate fills every provider-facing row. After that, the
    // requested model is either current in sqlite or genuinely absent.
    ensure_models_dev_catalog().await;
    if let Some(metadata) = current_cached_upstream_model_metadata(provider, model) {
        return Some(apply_overrides(provider, model, metadata));
    }

    override_metadata(provider, model)
}

pub(super) fn upstream_metadata_from_api(
    api: &Value,
    provider: &str,
    model: &str,
) -> Option<ModelMetadata> {
    let descriptor = crate::provider::provider_descriptor(provider)?;
    model_metadata_from_api_with_policy(
        api,
        descriptor.metadata_upstream_for_model(model),
        descriptor.metadata_model(model),
        descriptor.catalog_reasoning,
    )
}

fn apply_overrides(provider: &str, model: &str, metadata: ModelMetadata) -> ModelMetadata {
    let metadata = overrides::apply_builtin_overrides(provider, model, metadata);
    let metadata = apply_provider_capabilities(provider, model, metadata);
    overrides::apply_local_overrides(provider, model, metadata)
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
    metadata.display_name.is_some()
        || metadata.advertised_context_window.is_some()
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
    // Reuse the document for a full hydrate when this process has not already
    // marked the snapshot current, so a deprecation check also warms names.
    if !hydrate::catalog_snapshot_is_ready() {
        let _guard = hydrate::catalog_hydrate_lock_for_parent().lock().await;
        if !hydrate::catalog_snapshot_is_ready() {
            let written = hydrate::hydrate_catalog_from_api(&response);
            if written > 0 && hydrate::mark_catalog_snapshot_current() {
                hydrate::apply_in_memory_catalog_ready();
            }
        }
    }
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

pub(super) async fn fetch_models_dev_api() -> Option<Value> {
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

/// Bump when the models.dev parser gains fields that older cache rows omit.
/// Older or incomplete rows remain available as stale offline fallback, while
/// explicit fetch paths rehydrate and write them from a catalog snapshot.
///
/// v7: Qwen Token Plan switched from Unknown to ExactAdvertised. Rows written
/// under Unknown stored `reasoning_metadata_complete = true` with no levels, so
/// fetch short-circuited forever. Bump forces rehydrate from models.dev.
///
/// v8: `display_name` added. Older rows are complete without it, so only a bump
/// makes them refetch and pick up the catalog name.
///
/// `sdk_package` was added without a bump: only opencode-go reads it, and that
/// provider registered in the same release, so no older rows can miss it. Bump
/// when an already-registered provider switches to `PreferModelsDevNpm`.
pub(super) const MODEL_METADATA_CACHE_VERSION: i64 = 8;

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
    write_cached_upstream_model_metadata_raw(provider, model, metadata);
    super::display_name::forget_provider_display_names(provider);
}

/// Writes a batch of model metadata rows in a single SQLite transaction with a prepared statement.
pub(super) fn write_cached_upstream_model_metadata_batch<'a, I>(entries: I) -> usize
where
    I: IntoIterator<Item = (&'a str, &'a str, &'a ModelMetadata)>,
{
    let Ok(mut connection) = open_models_dev_cache() else {
        return 0;
    };
    let Ok(tx) = connection.transaction() else {
        return 0;
    };
    let mut written = 0;
    {
        let Ok(mut stmt) = tx.prepare_cached(
            "insert into model_metadata (provider, model, metadata_json, updated_at, cache_version)
             values (?1, ?2, ?3, strftime('%s', 'now'), ?4)
             on conflict(provider, model) do update set
               metadata_json = excluded.metadata_json,
               updated_at = excluded.updated_at,
               cache_version = excluded.cache_version",
        ) else {
            return 0;
        };
        for (provider, model, metadata) in entries {
            let Ok(contents) = serde_json::to_string(metadata) else {
                continue;
            };
            if stmt
                .execute(params![
                    provider,
                    model,
                    contents,
                    MODEL_METADATA_CACHE_VERSION
                ])
                .is_ok()
            {
                written += 1;
            }
        }
    }
    if tx.commit().is_err() {
        return 0;
    }
    written
}

/// Writes a row without invalidating the display-name cache.
///
/// Full hydrate touches many providers; callers invalidate each touched
/// provider once at the end instead of once per row.
pub(super) fn write_cached_upstream_model_metadata_raw(
    provider: &str,
    model: &str,
    metadata: &ModelMetadata,
) {
    write_cached_upstream_model_metadata_batch([(provider, model, metadata)]);
}

pub(super) fn open_models_dev_cache() -> rusqlite::Result<Connection> {
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
        );
        create table if not exists catalog_snapshot (
            id integer primary key check (id = 1),
            cache_version integer not null,
            updated_at integer not null
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

fn cache_dir() -> PathBuf {
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

thread_local! {
    static TEST_CACHE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(super) fn test_cache_dir_override_is_set() -> bool {
    TEST_CACHE_DIR.with(|cache_dir| cache_dir.borrow().is_some())
}

#[doc(hidden)]
pub fn with_models_dev_cache_dir_for_tests<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    // Process-level ready must not leak across tests that swap the sqlite path.
    reset_catalog_hydrate_state_for_tests();
    TEST_CACHE_DIR.with(|cache_dir| {
        let previous = cache_dir.replace(Some(path));
        let result = f();
        cache_dir.replace(previous);
        reset_catalog_hydrate_state_for_tests();
        result
    })
}

/// Holds the models.dev cache dir for the current thread across awaits.
///
/// Prefer [`with_models_dev_cache_dir_for_tests`] for sync work. Use this when a
/// test must keep the path set while awaiting on a `current_thread` runtime.
#[doc(hidden)]
pub struct ModelsDevCacheDirGuard {
    previous: Option<PathBuf>,
}

impl ModelsDevCacheDirGuard {
    #[doc(hidden)]
    pub fn new(path: PathBuf) -> Self {
        reset_catalog_hydrate_state_for_tests();
        let previous = TEST_CACHE_DIR.with(|cache_dir| cache_dir.replace(Some(path)));
        Self { previous }
    }
}

impl Drop for ModelsDevCacheDirGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        TEST_CACHE_DIR.with(|cache_dir| {
            cache_dir.replace(previous);
        });
        reset_catalog_hydrate_state_for_tests();
    }
}

#[doc(hidden)]
pub fn write_cached_model_metadata_for_tests(
    provider: &str,
    model: &str,
    metadata: &ModelMetadata,
) {
    write_cached_upstream_model_metadata(provider, model, metadata);
}

/// Marks the on-disk catalog snapshot current for tests that pre-seed rows.
#[doc(hidden)]
pub fn mark_catalog_snapshot_current_for_tests() {
    let _ = hydrate::mark_catalog_snapshot_current();
    hydrate::apply_in_memory_catalog_ready();
}

/// Ages the on-disk catalog snapshot past the freshness window for tests.
///
/// Leaves `cache_version` at the current binary value so only timestamp
/// staleness forces another hydrate.
#[doc(hidden)]
pub fn age_catalog_snapshot_for_tests() {
    hydrate::set_catalog_ready(false);
    let Ok(connection) = open_models_dev_cache() else {
        return;
    };
    let _ = connection.execute(
        "insert into catalog_snapshot (id, cache_version, updated_at)
         values (1, ?1, 0)
         on conflict(id) do update set
           cache_version = excluded.cache_version,
           updated_at = excluded.updated_at",
        params![MODEL_METADATA_CACHE_VERSION],
    );
}

/// Clears process-local hydrate readiness so a test can force another download path.
#[doc(hidden)]
pub fn reset_catalog_hydrate_state_for_tests() {
    hydrate::set_catalog_ready(false);
}

#[cfg(test)]
fn with_models_dev_cache_dir<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    with_models_dev_cache_dir_for_tests(path, f)
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
    let models = api.get(provider)?.get("models")?;
    let model = models
        .get(model)
        .or_else(|| models.get(model.strip_prefix("openai/")?))
        .or_else(|| models.get(format!("{provider}/{model}")))?;
    let limit = model.get("limit");
    let cost = model.get("cost");
    let (long_context_threshold, cost_long_context) = long_context_cost_from_api(cost);
    Some(ModelMetadata {
        display_name: model
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string),
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
        sdk_package: resolved_sdk_package(api.get(provider), model),
    })
}

fn reasoning_metadata_complete(model: &Value, policy: CatalogReasoningPolicy) -> bool {
    if matches!(
        policy,
        CatalogReasoningPolicy::Unknown
            | CatalogReasoningPolicy::NotConfigurable
            | CatalogReasoningPolicy::OffOrMax
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
    if policy == CatalogReasoningPolicy::OffOrMax {
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
    if effort_values(model)
        .is_some_and(|values| !values.is_empty() && values.iter().all(is_recognized_effort_value))
    {
        return true;
    }
    toggle_is_complete_binary_control(model, policy)
}

/// A toggle-only catalog row is a complete binary on/off control, but only for
/// protocols that already treat Off as an explicit wire value (`none`).
fn toggle_is_complete_binary_control(model: &Value, policy: CatalogReasoningPolicy) -> bool {
    advertised_toggle(model)
        && matches!(
            policy,
            CatalogReasoningPolicy::OffAsNone | CatalogReasoningPolicy::ExactAdvertised
        )
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
    if policy == CatalogReasoningPolicy::OffOrMax {
        return Some(vec![ReasoningLevel::Off, ReasoningLevel::Max]);
    }
    let supports_reasoning = model.get("reasoning")?.as_bool()?;
    if !supports_reasoning {
        return None;
    }
    let reasoning_options = model.get("reasoning_options").and_then(Value::as_array);
    if reasoning_options.is_some_and(Vec::is_empty) {
        return None;
    }
    let Some(effort_values) = effort_values(model) else {
        return toggle_is_complete_binary_control(model, policy)
            .then_some(vec![ReasoningLevel::Off, ReasoningLevel::Max]);
    };
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
