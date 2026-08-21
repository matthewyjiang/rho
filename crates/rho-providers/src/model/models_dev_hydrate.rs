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
use tokio::sync::Mutex;

use crate::provider::{CatalogConstruction, CatalogLookupMode, ProviderDescriptor, ProviderId};

use super::{
    document::{self, ModelsDevCatalog},
    fetch_models_dev_api, model_metadata_needs_refresh, open_models_dev_cache,
    upstream_metadata_from_api, write_cached_upstream_model_metadata_batch,
    MODEL_ID_CATALOG_CACHE_PROVIDER, MODEL_METADATA_CACHE_VERSION,
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
/// (same cache version, fresh `updated_at`, and every interned borrowed catalog
/// slug already written) is free. A host that `/login` or a config edit points
/// at a new slug after that snapshot was taken is not free: the next ensure
/// redownloads so context, price, and reasoning do not wait 24 hours.
/// Returns how many rows the hydrate wrote, or `0` when the network was
/// skipped or failed.
pub async fn ensure_models_dev_catalog() -> usize {
    if catalog_snapshot_is_ready() {
        return 0;
    }
    let _guard = catalog_hydrate_lock().lock().await;
    if catalog_snapshot_is_ready() {
        return 0;
    }
    hydrate_catalog_from_network().await
}

/// Downloads models.dev again, ignoring the 24h snapshot gate.
///
/// `/config` Providers uses this so a host that just switched to
/// `catalog_mode = "model-id"` can fill arbitrary slugs without waiting.
pub async fn force_refresh_models_dev_catalog() -> usize {
    let _guard = catalog_hydrate_lock().lock().await;
    invalidate_catalog_snapshot();
    hydrate_catalog_from_network().await
}

async fn hydrate_catalog_from_network() -> usize {
    let Some(response) = fetch_models_dev_api().await else {
        return 0;
    };
    let written = hydrate_catalog_from_api(&response);
    if written > 0 && mark_catalog_snapshot_current() {
        CATALOG_READY.store(true, Ordering::Release);
    }
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
///
/// models.dev slugs that interned custom hosts actually borrow (`catalog =
/// "llmgateway"` or `catalog = "openrouter"`) are also written so those hosts
/// can rematch cache rows by slug and model id. A slug that is also a built-in
/// cache key (`openrouter`) is written under the borrowing host name instead,
/// so extract for Rho's own provider is unchanged. Slugs with no document
/// (`openai-codex`) keep extract and rematch.
///
/// Any interned `catalog_mode = "model-id"` host additionally needs the entire
/// models.dev tree written once under [`MODEL_ID_CATALOG_CACHE_PROVIDER`] with
/// unsplit `slug/model` ids, parsed with that mode's hydrate policy. Unborrowed
/// upstream providers otherwise stay out of sqlite.
pub(super) fn hydrate_catalog_from_api(api: &ModelsDevCatalog) -> usize {
    let mut entries = Vec::new();
    let mut touched_providers = HashSet::new();
    for descriptor in crate::provider::providers() {
        for model_id in catalog_model_ids_for_provider(api, descriptor) {
            if let Some(metadata) = extract_complete_upstream_metadata(api, descriptor, &model_id) {
                touched_providers.insert(descriptor.name.to_string());
                entries.push((descriptor.name.to_string(), model_id, metadata));
            }
        }
        // Provider-facing ids that are not catalog keys still need a cache row.
        if descriptor.id == ProviderId::KimiCode {
            if let Some(metadata) = extract_complete_upstream_metadata(api, descriptor, "k3") {
                touched_providers.insert(descriptor.name.to_string());
                entries.push((descriptor.name.to_string(), "k3".to_string(), metadata));
            }
        }
    }
    for host in crate::provider::interned_custom_providers() {
        let slug = host.metadata_upstream;
        if slug == host.name {
            continue;
        }
        let Some(provider) = api.provider(slug) else {
            continue;
        };
        let cache_provider = if super::borrowed_slug_collides_with_builtin_extract(slug) {
            host.name
        } else {
            slug
        };
        for model_id in provider.models.keys() {
            let Some(metadata) =
                document::model_metadata_from_catalog(api, slug, model_id, host.catalog_reasoning)
            else {
                continue;
            };
            touched_providers.insert(cache_provider.to_string());
            entries.push((cache_provider.to_string(), model_id.clone(), metadata));
        }
    }
    let extra = needed_extra_catalog_docs();
    if extra.full_tree {
        let policy = CatalogLookupMode::MODEL_ID_HYDRATE_REASONING;
        for (slug, provider) in api.iter_providers() {
            for model_id in provider.models.keys() {
                let Some(metadata) =
                    document::model_metadata_from_catalog(api, slug, model_id, policy)
                        .filter(|metadata| metadata.reasoning_metadata_complete)
                else {
                    continue;
                };
                entries.push((
                    MODEL_ID_CATALOG_CACHE_PROVIDER.to_string(),
                    format!("{slug}/{model_id}"),
                    metadata,
                ));
            }
        }
    }
    let written = write_cached_upstream_model_metadata_batch(
        entries
            .iter()
            .map(|(provider, model, metadata)| (provider.as_str(), model.as_str(), metadata)),
    );
    if written > 0 {
        if extra.full_tree {
            // Display names are cached under the host that looks them up, not
            // the shared tree key.
            for host in crate::provider::interned_custom_providers() {
                if host.catalog_lookup() == CatalogLookupMode::ModelId {
                    touched_providers.insert(host.name.to_string());
                }
            }
        }
        for provider in touched_providers {
            crate::model::display_name::forget_provider_display_names(&provider);
        }
    }
    written
}

fn catalog_model_ids_for_provider(
    api: &ModelsDevCatalog,
    descriptor: &crate::provider::ProviderDescriptor,
) -> Vec<String> {
    let upstream = match descriptor.id {
        // OpenRouter model ids are aggregator-qualified and live under the
        // openrouter document, even though per-model lookup reads the upstream
        // provider section for reasoning/name fields.
        ProviderId::OpenRouter => "openrouter",
        _ => descriptor.metadata_upstream,
    };
    api.provider(upstream)
        .map(|provider| provider.models.keys().cloned().collect())
        .unwrap_or_default()
}

/// Extra models.dev rows this snapshot must contain besides Rho extract.
///
/// Two independent dimensions: borrowed slug documents written slug-keyed
/// (`catalog = "llmgateway"`), and one shared ExactAdvertised `slug/model`
/// tree for any interned `catalog_mode = "model-id"` host. A snapshot is only
/// reusable when it covers both, so a new borrow or the first model-id host
/// refetches. A second model-id host reuses the same tree.
#[derive(Default, PartialEq, Eq)]
struct ExtraCatalogDocs {
    /// Borrowed models.dev slugs written under the slug (or borrowing host on
    /// collision) by the borrow loop.
    slugs: HashSet<String>,
    /// Shared tree keyed [`MODEL_ID_CATALOG_CACHE_PROVIDER`].
    full_tree: bool,
}

/// Sentinel stored in `catalog_snapshot.borrowed_slugs` for the shared tree.
const EXTRA_CATALOG_DOCS_FULL_TREE: &str = "*";

fn needed_extra_catalog_docs() -> ExtraCatalogDocs {
    let mut docs = ExtraCatalogDocs::default();
    for descriptor in crate::provider::interned_custom_providers() {
        if descriptor.catalog_lookup() == CatalogLookupMode::ModelId {
            docs.full_tree = true;
        } else if descriptor.metadata_upstream != descriptor.name {
            docs.slugs.insert(descriptor.metadata_upstream.to_string());
        }
    }
    docs
}

fn encode_extra_catalog_docs(docs: &ExtraCatalogDocs) -> String {
    let mut items = docs.slugs.iter().cloned().collect::<Vec<_>>();
    if docs.full_tree {
        items.push(EXTRA_CATALOG_DOCS_FULL_TREE.to_string());
    }
    items.sort_unstable();
    items.join(",")
}

fn decode_extra_catalog_docs(raw: &str) -> ExtraCatalogDocs {
    let mut docs = ExtraCatalogDocs::default();
    for item in raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        if item == EXTRA_CATALOG_DOCS_FULL_TREE {
            docs.full_tree = true;
        } else {
            docs.slugs.insert(item.to_string());
        }
    }
    docs
}

fn stored_extra_catalog_docs() -> ExtraCatalogDocs {
    let Ok(connection) = open_models_dev_cache() else {
        return ExtraCatalogDocs::default();
    };
    connection
        .query_row(
            "select borrowed_slugs from catalog_snapshot where id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .map(|raw| decode_extra_catalog_docs(&raw))
        .unwrap_or_default()
}

fn extra_catalog_docs_are_hydrated() -> bool {
    extra_catalog_docs_cover(&needed_extra_catalog_docs(), &stored_extra_catalog_docs())
}

fn extra_catalog_docs_cover(needed: &ExtraCatalogDocs, stored: &ExtraCatalogDocs) -> bool {
    needed.slugs.is_subset(&stored.slugs) && (!needed.full_tree || stored.full_tree)
}

/// Extracts metadata when its reasoning metadata is complete. Providers whose
/// construction follows the catalog's npm mapping also keep
/// reasoning-incomplete rows, because the builder needs `sdk_package` even
/// when reasoning levels stay unknown.
fn extract_complete_upstream_metadata(
    api: &ModelsDevCatalog,
    descriptor: &ProviderDescriptor,
    model: &str,
) -> Option<super::ModelMetadata> {
    let keep_sdk_only_rows =
        descriptor.runtime.catalog_construction() == CatalogConstruction::PreferModelsDevNpm;
    upstream_metadata_from_api(api, descriptor.name, model).filter(|metadata| {
        metadata.reasoning_metadata_complete
            || (keep_sdk_only_rows && metadata.sdk_package.is_some())
    })
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
    if !extra_catalog_docs_are_hydrated() {
        return false;
    }
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

pub(super) fn mark_catalog_snapshot_current() -> bool {
    let Ok(connection) = open_models_dev_cache() else {
        return false;
    };
    let extra = encode_extra_catalog_docs(&needed_extra_catalog_docs());
    connection
        .execute(
            "insert into catalog_snapshot (id, cache_version, updated_at, borrowed_slugs)
             values (1, ?1, strftime('%s', 'now'), ?2)
             on conflict(id) do update set
               cache_version = excluded.cache_version,
               updated_at = excluded.updated_at,
               borrowed_slugs = excluded.borrowed_slugs",
            params![MODEL_METADATA_CACHE_VERSION, extra],
        )
        .is_ok()
}

pub(super) fn invalidate_catalog_snapshot() {
    set_catalog_ready(false);
    let Ok(connection) = open_models_dev_cache() else {
        return;
    };
    let _ = connection.execute(
        "insert into catalog_snapshot (id, cache_version, updated_at, borrowed_slugs)
         values (1, ?1, 0, '')
         on conflict(id) do update set
           cache_version = excluded.cache_version,
           updated_at = excluded.updated_at,
           borrowed_slugs = excluded.borrowed_slugs",
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
