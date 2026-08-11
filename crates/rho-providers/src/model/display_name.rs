//! Catalog names for models, for text that people and models read.
//!
//! A model id such as `gpt-5.6-sol` is the fact Rho acts on: it selects the
//! provider route, and it is what a user types back into `/model`. The name is
//! the fact a person recognizes. Both come from caches Rho already fills, so
//! nothing here reaches the network, and nothing here invents a name from an id:
//! an unknown model shows its id alone rather than a guess.

use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

use crate::model::{models_dev, provider_models};

/// Names resolved this process, including the models that have none.
///
/// Every lookup otherwise opens a fresh sqlite connection and runs its schema
/// statements, and these lookups sit on hot paths: every delegated run listing
/// formats one.
///
/// A cached answer is dropped when a catalog write lands for that provider, so
/// a name that arrives during a session reaches the next text that names the
/// model. Text already produced is never revisited: an entry only changes when
/// the catalog underneath it does.
type NameCache = HashMap<(String, String), Option<String>>;

fn cache() -> &'static RwLock<NameCache> {
    static CACHE: OnceLock<RwLock<NameCache>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(NameCache::new()))
}

/// Catalog name for `provider/model`, or `None` when no catalog carries one.
///
/// The models.dev catalog wins because its names are curated across providers
/// (`GPT-5.6 Sol`, `Claude Fable 5`). A provider's own model list is the
/// fallback, and only when it named the model: discovery stores the id as the
/// display name for providers that publish no name, and echoing the id back as
/// a name would be noise.
///
/// Resolved once per process; see [`NameCache`].
pub fn model_display_name(provider: &str, model: &str) -> Option<String> {
    let key = (provider.to_string(), model.to_string());
    if let Some(name) = cache().read().expect("model name cache").get(&key) {
        return name.clone();
    }
    let name = read_model_display_name(provider, model);
    cache()
        .write()
        .expect("model name cache")
        .insert(key, name.clone());
    name
}

fn read_model_display_name(provider: &str, model: &str) -> Option<String> {
    if let Some(name) = models_dev::cached_model_metadata(provider, model)
        .and_then(|metadata| metadata.display_name)
    {
        return Some(name);
    }
    provider_models::cached_provider_model(provider, model)
        .map(|entry| entry.display_name)
        .filter(|name| name != model)
}

/// Drops this provider's resolved names so the next lookup reads the catalog.
///
/// Every catalog write calls this. Without it a lookup that missed keeps its
/// `None` for the rest of the process, which defeats the startup prefetch on the
/// launch that needs it: the system prompt asks for every name it wants before
/// a download can finish, so the names would first appear on the next launch.
pub(crate) fn forget_provider_display_names(provider: &str) {
    cache()
        .write()
        .expect("model name cache")
        .retain(|(cached_provider, _), _| cached_provider != provider);
}

/// Drops resolved names so a test can write a catalog row and read it back.
#[doc(hidden)]
pub fn clear_model_display_name_cache_for_tests() {
    cache().write().expect("model name cache").clear();
}

/// `provider/model (Catalog Name)`, or `provider/model` when no name is known.
///
/// The id stays first and always present: it is the part a caller can act on.
pub fn model_reference_with_display_name(provider: &str, model: &str) -> String {
    let reference = crate::provider::model_reference(provider, model);
    match model_display_name(provider, model) {
        Some(name) => format!("{reference} ({name})"),
        None => reference,
    }
}

#[cfg(test)]
#[path = "display_name_tests.rs"]
mod tests;
