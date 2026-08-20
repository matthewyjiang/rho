use std::{cell::RefCell, collections::HashSet, fs, path::PathBuf, time::Duration};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;

use crate::{
    model::ReasoningCapabilities,
    provider::{CatalogLookupMode, CatalogReasoningPolicy},
    reasoning::ReasoningLevel,
};

#[path = "models_dev_document.rs"]
mod document;
#[path = "models_dev_hydrate.rs"]
mod hydrate;
#[path = "models_dev_overrides.rs"]
mod overrides;
#[path = "models_dev_sdk.rs"]
mod sdk;
pub use hydrate::{
    ensure_models_dev_catalog, force_refresh_models_dev_catalog, prefetch_model_metadata,
};

/// Holds the catalog hydrate mutex so tests can prove a caller does not await it.
#[doc(hidden)]
pub fn catalog_hydrate_lock_for_tests() -> &'static tokio::sync::Mutex<()> {
    hydrate::catalog_hydrate_lock_for_parent()
}
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
    load_model_metadata(provider, model, CacheFreshness::CurrentOnly)
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
    load_model_metadata(provider, model, CacheFreshness::CurrentOnly)
        .or_else(|| override_metadata(provider, model))
        .is_none_or(|metadata| !metadata.reasoning_metadata_complete)
}

pub fn cached_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    load_model_metadata(provider, model, CacheFreshness::AllowStale)
        .or_else(|| override_metadata(provider, model))
}

pub async fn fetch_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    if let Some(metadata) = load_model_metadata(provider, model, CacheFreshness::CurrentOnly) {
        return Some(metadata);
    }

    // One full catalog hydrate fills every provider-facing row. After that, the
    // requested model is either current in sqlite or genuinely absent.
    ensure_models_dev_catalog().await;
    if let Some(metadata) = load_model_metadata(provider, model, CacheFreshness::CurrentOnly) {
        return Some(metadata);
    }

    override_metadata(provider, model)
}

/// Why a custom host with `catalog_mode = "model-id"` has no models.dev row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogLookupMiss {
    /// The selected model id has no `provider/model` slash.
    BareModelId,
    /// The id split, but that models.dev pair is absent.
    MissingRow {
        source_provider: String,
        source_model: String,
    },
}

/// Some when this custom host splits model ids and catalog metadata is absent.
pub fn custom_model_id_catalog_miss(provider: &str, model: &str) -> Option<CatalogLookupMiss> {
    let descriptor = crate::provider::provider_descriptor(provider)?;
    if !descriptor.is_custom_openai_compatible() {
        return None;
    }
    if descriptor.catalog_lookup != CatalogLookupMode::ModelId {
        return None;
    }
    if cached_model_metadata(provider, model).is_some() {
        return None;
    }
    Some(match overrides::split_provider_model(model) {
        None => CatalogLookupMiss::BareModelId,
        Some((source_provider, source_model)) => {
            if !hydrate::catalog_snapshot_is_ready() {
                return None;
            }
            CatalogLookupMiss::MissingRow {
                source_provider,
                source_model,
            }
        }
    })
}

/// models.dev provider and model id used for the sqlite catalog row.
///
/// Custom hosts can borrow another slug (`catalog = "llmgateway"`) or split
/// the selected model id (`catalog_mode = "model-id"`). A models.toml
/// `catalog` value wins and may also remap the model id (`anthropic/claude-sonnet-4-5`).
///
/// Built-in providers are deliberately left on their provider-facing name:
/// hydrate writes their rows under that name, not under `metadata_upstream`
/// (`openai-codex` rows, not `openai`), so borrowing the upstream slug here
/// would miss the cache. Only config-defined hosts redirect, and only when
/// that slug is not itself a built-in cache key. `catalog = "openrouter"`
/// rows live under the custom host so they cannot replace Rho OpenRouter.
fn catalog_source_for(
    provider: &str,
    model: &str,
    local: Option<&toml::map::Map<String, toml::Value>>,
) -> (String, String) {
    if let Some(source) = local.and_then(|table| overrides::local_catalog_source(table, model)) {
        return source;
    }
    let Some(descriptor) = crate::provider::provider_descriptor(provider)
        .filter(|descriptor| descriptor.is_custom_openai_compatible())
    else {
        return (provider.to_string(), model.to_string());
    };
    match descriptor.catalog_lookup {
        CatalogLookupMode::ModelId => overrides::split_provider_model(model)
            .unwrap_or_else(|| (provider.to_string(), model.to_string())),
        CatalogLookupMode::Slug => {
            // The catalog string is the models.dev provider key. Do not run built-in
            // remappers (OpenRouter's owner/model split) on a borrowed slug. A slug
            // that is also a built-in cache key *and* that provider's document
            // (`openrouter`) is keyed by the host so extract is untouched.
            // `openai-codex` is a built-in name whose document is `openai`, so those
            // hosts still rematch the Codex extract rows.
            let upstream = descriptor.metadata_upstream;
            if upstream != provider && !borrowed_slug_collides_with_builtin_extract(upstream) {
                (upstream.to_string(), model.to_string())
            } else {
                (provider.to_string(), model.to_string())
            }
        }
    }
}

/// True when writing borrowed rows under `slug` would collide with a built-in
/// provider's extract keys (`openrouter`). Those rows go under the host name.
/// `openai-codex` is a built-in name whose document is `openai`, so it still
/// rematches extract rows and is not a collision.
fn borrowed_slug_collides_with_builtin_extract(slug: &str) -> bool {
    crate::provider::providers()
        .iter()
        .any(|descriptor| descriptor.name == slug && descriptor.metadata_upstream == slug)
}

fn load_model_metadata(
    provider: &str,
    model: &str,
    freshness: CacheFreshness,
) -> Option<ModelMetadata> {
    let local = overrides::local_override_table(provider, model);
    let (source_provider, source_model) = catalog_source_for(provider, model, local.as_ref());
    let remapped = source_provider != provider || source_model != model;
    let metadata = match freshness {
        CacheFreshness::CurrentOnly => {
            current_cached_upstream_model_metadata(&source_provider, &source_model)
        }
        CacheFreshness::AllowStale => {
            cached_upstream_model_metadata(&source_provider, &source_model)
        }
    }?;
    let metadata = if remapped {
        overrides::apply_builtin_overrides(&source_provider, &source_model, metadata)
    } else {
        overrides::apply_builtin_overrides(provider, model, metadata)
    };
    let metadata = apply_provider_capabilities(provider, model, metadata);
    Some(match local.as_ref() {
        Some(table) => overrides::merge_toml_override(metadata, table),
        None => metadata,
    })
}

fn upstream_metadata_from_api(
    api: &document::ModelsDevCatalog,
    provider: &str,
    model: &str,
) -> Option<ModelMetadata> {
    let descriptor = crate::provider::provider_descriptor(provider)?;
    document::model_metadata_from_catalog(
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
    Some(document::deprecated_provider_models(&response, provider))
}

async fn fetch_models_dev_api() -> Option<document::ModelsDevCatalog> {
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
        .json::<document::ModelsDevCatalog>()
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
/// v9: hydrate also writes non-Rho models.dev slugs so custom hosts can set
/// `catalog = "llmgateway"` and borrow those rows. A version match on an older
/// snapshot would otherwise skip the download forever.
///
/// `sdk_package` was added without a bump: only opencode-go reads it, and that
/// provider registered in the same release, so no older rows can miss it. Bump
/// when an already-registered provider switches to `PreferModelsDevNpm`.
pub(super) const MODEL_METADATA_CACHE_VERSION: i64 = 9;

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
            updated_at integer not null,
            borrowed_slugs text not null default ''
        );",
    )?;
    let _ = connection.execute(
        "alter table model_metadata add column cache_version integer not null default 1",
        [],
    );
    let _ = connection.execute(
        "alter table catalog_snapshot add column borrowed_slugs text not null default ''",
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
    hydrate::invalidate_catalog_snapshot();
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
    document::model_metadata_from_catalog(
        &document::ModelsDevCatalog::from_json_value(api),
        provider,
        model,
        policy,
    )
}

#[cfg(test)]
#[path = "models_dev_tests.rs"]
mod tests;
