//! Model ids the Claude Code CLI reported for the `--model` values Rho gave it.
//!
//! Rho passes `--model` through untouched, and the `claude` binary has no
//! command that enumerates models (see [`super::models`]). So `opus` is a
//! pointer Rho cannot follow: the only place the model behind it appears is the
//! `system`/`init` frame at the start of a run.
//!
//! This store keeps what those frames reported, so prompt text can name the
//! model an alias actually ran as instead of repeating the alias back. It is
//! deliberately process-local and not persisted: an alias points at whichever
//! model is current, and a mapping saved to disk would outlive that and name a
//! retired model with confidence. Unrecorded is the honest state until a run
//! reports one, and every run refreshes what it reports.

use std::{
    collections::BTreeMap,
    sync::{OnceLock, RwLock},
};

/// Resolutions seen this process, keyed by the `--model` value Rho passed.
///
/// `default_model` holds the run that passed no `--model` at all, which cannot
/// share the map without a sentinel key that a real model name could collide
/// with.
#[derive(Default)]
struct ResolvedModels {
    aliases: BTreeMap<String, String>,
    default_model: Option<String>,
}

fn store() -> &'static RwLock<ResolvedModels> {
    static STORE: OnceLock<RwLock<ResolvedModels>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(ResolvedModels::default()))
}

/// Records that a run started with `requested` reported running `resolved`.
///
/// `requested` is `None` when Rho omitted `--model` and let Claude choose.
/// Blank reports are dropped rather than stored as an empty name.
pub(crate) fn record(requested: Option<&str>, resolved: &str) {
    let resolved = resolved.trim();
    if resolved.is_empty() {
        return;
    }
    let mut store = store().write().expect("resolved Claude model lock");
    match requested {
        Some(requested) => {
            store
                .aliases
                .insert(requested.to_string(), resolved.to_string());
        }
        None => store.default_model = Some(resolved.to_string()),
    }
}

/// Records the resolution a stream effect carries, if it carries one.
///
/// Both Claude drain paths (delegated sessions and one-shots) pass every effect
/// through here, so a run reports its model exactly once wherever it runs.
pub(crate) fn note_stream_effect(requested: Option<&str>, effect: &super::stream::StreamEffect) {
    let super::stream::StreamEffect::Status(patch) = effect else {
        return;
    };
    if let Some(model) = &patch.claude_model {
        record(requested, model);
    }
}

/// The model a `--model` value last ran as, or `None` when no run reported one.
pub(crate) fn last_resolved(requested: Option<&str>) -> Option<String> {
    let store = store().read().expect("resolved Claude model lock");
    match requested {
        Some(requested) => store.aliases.get(requested).cloned(),
        None => store.default_model.clone(),
    }
}

#[cfg(test)]
pub(crate) fn clear_for_tests() {
    let mut store = store().write().expect("resolved Claude model lock");
    store.aliases.clear();
    store.default_model = None;
}

/// Serializes every test that reads or writes this process-wide store.
///
/// One lock for all of them: two locks guarding the same store would let tests
/// in different modules interleave writes and read each other's models.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "resolved_models_tests.rs"]
mod tests;
