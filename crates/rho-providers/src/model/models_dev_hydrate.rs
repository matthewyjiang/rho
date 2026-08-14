//! Full models.dev snapshot hydrate and process-local readiness.
//!
//! Owned as a child module so `models_dev` stays focused on row parse/cache.

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::params;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::provider::ProviderId;

use super::{
    fetch_models_dev_api, model_metadata_needs_refresh, open_models_dev_cache,
    upstream_metadata_from_api, write_cached_upstream_model_metadata_raw,
    MODEL_METADATA_CACHE_VERSION,
};

/// How long a successful full-catalog snapshot stays current across launches.
///
/// A version match alone is not enough: models.dev gains rows without a Rho
/// binary bump. Within one process the in-memory ready flag still suppresses
/// duplicate downloads; later launches recheck this window.
const CATALOG_SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Ensures the models.dev snapshot is on disk for this process.
///
/// The models.dev document is one full catalog. Writing only the caller's
/// targets forced every permanent-text site to know which keys to warm and to
/// race a background prefetch. A successful download hydrates every complete
/// row for every registered Rho provider instead.
///
/// Concurrent callers share one in-flight download. A current on-disk snapshot
/// (same cache version and fresh `updated_at`) is free. Returns how many rows
/// the hydrate wrote, or `0` when the network was skipped or failed.
pub async fn ensure_models_dev_catalog() -> usize {
    if catalog_snapshot_is_ready() {
        return 0;
    }
    let _guard = catalog_hydrate_lock().lock().await;
    if catalog_snapshot_is_ready() {
        return 0;
    }
    let Some(response) = fetch_models_dev_api().await else {
        return 0;
    };
    let written = hydrate_catalog_from_api(&response);
    mark_catalog_snapshot_current();
    CATALOG_READY.store(true, Ordering::Release);
    written
}

/// Fills catalog rows for several models with one models.dev download.
///
/// Prefer [`ensure_models_dev_catalog`]: the download is a full snapshot, so
/// target lists no longer gate what gets written. This keeps the old name for
/// callers that still pass session models and short-circuits when every target
/// is already current *and* a full snapshot is marked current.
///
/// Returns the number of rows the hydrate wrote.
pub async fn prefetch_model_metadata(targets: impl IntoIterator<Item = (String, String)>) -> usize {
    let targets = targets.into_iter().collect::<HashSet<_>>();
    if !targets.is_empty()
        && targets
            .iter()
            .all(|(provider, model)| !model_metadata_needs_refresh(provider, model))
        && catalog_snapshot_is_ready()
    {
        return 0;
    }
    ensure_models_dev_catalog().await
}

/// Writes every complete models.dev row for every registered Rho provider.
///
/// Cache keys stay provider-facing (`openai-codex` / model), not upstream. OpenRouter
/// rows use the aggregator model ids (`anthropic/claude-…`). Kimi Code's `k3`
/// alias is written beside the upstream `kimi-k3` id.
pub(super) fn hydrate_catalog_from_api(api: &Value) -> usize {
    let mut written = 0;
    let mut touched_providers = HashSet::new();
    for descriptor in crate::provider::providers() {
        for model_id in catalog_model_ids_for_provider(api, descriptor) {
            if write_complete_upstream_row(api, descriptor.name, &model_id) {
                touched_providers.insert(descriptor.name);
                written += 1;
            }
        }
        // Provider-facing ids that are not catalog keys still need a cache row.
        if descriptor.id == ProviderId::KimiCode
            && write_complete_upstream_row(api, descriptor.name, "k3")
        {
            touched_providers.insert(descriptor.name);
            written += 1;
        }
    }
    for provider in touched_providers {
        crate::model::display_name::forget_provider_display_names(provider);
    }
    written
}

fn catalog_model_ids_for_provider(
    api: &Value,
    descriptor: &crate::provider::ProviderDescriptor,
) -> Vec<String> {
    let upstream = match descriptor.id {
        // OpenRouter model ids are aggregator-qualified and live under the
        // openrouter document, even though per-model lookup reads the upstream
        // provider section for reasoning/name fields.
        ProviderId::OpenRouter => "openrouter",
        _ => descriptor.metadata_upstream,
    };
    api.get(upstream)
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_object)
        .map(|models| models.keys().cloned().collect())
        .unwrap_or_default()
}

fn write_complete_upstream_row(api: &Value, provider: &str, model: &str) -> bool {
    let Some(metadata) = upstream_metadata_from_api(api, provider, model)
        .filter(|metadata| metadata.reasoning_metadata_complete || metadata.sdk_package.is_some())
    else {
        return false;
    };
    write_cached_upstream_model_metadata_raw(provider, model, &metadata);
    true
}

/// Process-local "full catalog hydrate already succeeded this run".
///
/// Combined with the on-disk snapshot marker so warm sqlite from a previous
/// launch does not force another download, and concurrent callers share work.
static CATALOG_READY: AtomicBool = AtomicBool::new(false);

fn catalog_hydrate_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn catalog_snapshot_is_ready() -> bool {
    // Isolated test cache dirs share this process flag. A sibling test that
    // marked the catalog ready would otherwise skip our aged sqlite.
    if super::test_cache_dir_override_is_set() {
        return is_catalog_snapshot_current();
    }
    if CATALOG_READY.load(Ordering::Acquire) {
        return true;
    }
    is_catalog_snapshot_current()
}

fn is_catalog_snapshot_current() -> bool {
    let Ok(connection) = open_models_dev_cache() else {
        return false;
    };
    let Ok((version, updated_at)) = connection.query_row(
        "select cache_version, updated_at from catalog_snapshot where id = 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    ) else {
        return false;
    };
    version == MODEL_METADATA_CACHE_VERSION && catalog_snapshot_timestamp_is_fresh(updated_at)
}

fn catalog_snapshot_timestamp_is_fresh(updated_at: i64) -> bool {
    let Ok(max_age) = i64::try_from(CATALOG_SNAPSHOT_MAX_AGE.as_secs()) else {
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

pub(super) fn mark_catalog_snapshot_current() {
    let Ok(connection) = open_models_dev_cache() else {
        return;
    };
    let _ = connection.execute(
        "insert into catalog_snapshot (id, cache_version, updated_at)
         values (1, ?1, strftime('%s', 'now'))
         on conflict(id) do update set
           cache_version = excluded.cache_version,
           updated_at = excluded.updated_at",
        params![MODEL_METADATA_CACHE_VERSION],
    );
}

pub(super) fn catalog_hydrate_lock_for_parent() -> &'static Mutex<()> {
    catalog_hydrate_lock()
}

pub(super) fn set_catalog_ready(ready: bool) {
    CATALOG_READY.store(ready, Ordering::Release);
}

pub(super) fn apply_in_memory_catalog_ready() {
    CATALOG_READY.store(true, Ordering::Release);
}
