use std::collections::{btree_map::Entry, BTreeMap};

use crate::model::{ReasoningCapabilities, ReasoningLevelSet};
use crate::reasoning::ReasoningLevel;

use super::effort::{split_effort, strip_effort_display_suffix};
use super::fast::{catalog_model_id, strip_fast_suffix};
use super::proto::ModelDetails;

pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 64_000;

const FALLBACK_ROWS: &[(&str, &str, u64, u64)] = &[
    (
        "composer-1",
        "Composer 1",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "composer-1.5",
        "Composer 1.5",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "claude-4.6-opus-high",
        "Claude 4.6 Opus",
        DEFAULT_CONTEXT_WINDOW,
        128_000,
    ),
    (
        "claude-4.6-sonnet-medium",
        "Claude 4.6 Sonnet",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "claude-4.5-sonnet",
        "Claude 4.5 Sonnet",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    ("gpt-5.4-medium", "GPT-5.4", 272_000, 128_000),
    ("gpt-5.2", "GPT-5.2", 400_000, 128_000),
    ("gpt-5.2-codex", "GPT-5.2 Codex", 400_000, 128_000),
    ("gpt-5.3-codex", "GPT-5.3 Codex", 400_000, 128_000),
    (
        "gpt-5.3-codex-spark-preview",
        "GPT-5.3 Codex Spark",
        128_000,
        128_000,
    ),
    (
        "gemini-3.1-pro",
        "Gemini 3.1 Pro",
        1_000_000,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "grok-4.6-low",
        "Grok 4.6",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "grok-4.6-medium",
        "Grok 4.6",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "grok-4.6-high",
        "Grok 4.6",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "grok-4.6-xhigh",
        "Grok 4.6",
        DEFAULT_CONTEXT_WINDOW,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
    (
        "grok-code-fast-1",
        "Grok Code Fast 1",
        128_000,
        DEFAULT_MAX_OUTPUT_TOKENS,
    ),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorModel {
    pub id: String,
    pub name: String,
    pub reasoning_levels: Vec<ReasoningLevel>,
    pub context_window: u64,
    pub max_tokens: u64,
}

impl CursorModel {
    pub(crate) fn reasoning_capabilities(&self) -> ReasoningCapabilities {
        if self.reasoning_levels.is_empty() {
            ReasoningCapabilities::NotConfigurable
        } else {
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(self.reasoning_levels.clone()))
        }
    }
}

struct DiscoveredCursorModel {
    id: String,
    name: String,
    context_window: u64,
    max_tokens: u64,
}

pub(crate) fn fallback_models() -> Vec<CursorModel> {
    collapse_models(
        FALLBACK_ROWS
            .iter()
            .copied()
            .map(|(id, name, context_window, max_tokens)| {
                discovered(id, name, context_window, max_tokens)
            }),
    )
}

pub(crate) fn models_from_details(details: &[ModelDetails]) -> Vec<CursorModel> {
    let mut models = collapse_models(details.iter().filter_map(|details| {
        let id = details.model_id.trim();
        if id.is_empty() {
            return None;
        }
        let name = [
            details.display_name.as_str(),
            details.display_name_short.as_str(),
            details.display_model_id.as_str(),
        ]
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or(id);
        Some(discovered(
            id,
            name,
            DEFAULT_CONTEXT_WINDOW,
            DEFAULT_MAX_OUTPUT_TOKENS,
        ))
    }));
    for model in &mut models {
        if let Some((context_window, max_tokens)) = known_model_limits(&model.id) {
            model.context_window = context_window;
            model.max_tokens = max_tokens;
        }
    }
    models
}

fn known_model_limits(id: &str) -> Option<(u64, u64)> {
    FALLBACK_ROWS
        .iter()
        .find_map(|(raw, _, context_window, max_tokens)| {
            (catalog_model_id(raw) == id).then_some((*context_window, *max_tokens))
        })
}

fn discovered(id: &str, name: &str, context_window: u64, max_tokens: u64) -> DiscoveredCursorModel {
    DiscoveredCursorModel {
        id: id.into(),
        name: name.into(),
        context_window,
        max_tokens,
    }
}

/// Collapse Fast and effort suffixes into one catalog row per stem.
///
/// Detected effort tokens become the only reasoning levels the picker exposes.
fn collapse_models(rows: impl IntoIterator<Item = DiscoveredCursorModel>) -> Vec<CursorModel> {
    let mut by_id = BTreeMap::new();
    for row in rows {
        let raw_id = row.id.trim();
        if raw_id.is_empty() {
            continue;
        }
        let (without_fast, from_fast) = strip_fast_suffix(raw_id);
        let (catalog, effort) = split_effort(without_fast);
        let mut name = row.name.trim().to_string();
        if from_fast {
            name = name.strip_suffix(" Fast").unwrap_or(&name).to_string();
        }
        if effort.is_some() {
            name = strip_effort_display_suffix(&name).to_string();
        }
        match by_id.entry(catalog.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert((
                    CursorModel {
                        id: catalog.to_string(),
                        name,
                        reasoning_levels: effort.into_iter().collect(),
                        context_window: row.context_window,
                        max_tokens: row.max_tokens,
                    },
                    from_fast,
                ));
            }
            Entry::Occupied(mut entry) => {
                let (existing, existing_from_fast) = entry.get_mut();
                if let Some(level) = effort {
                    if !existing.reasoning_levels.contains(&level) {
                        existing.reasoning_levels.push(level);
                    }
                }
                if !from_fast {
                    existing.name = name;
                    *existing_from_fast = false;
                } else if *existing_from_fast {
                    existing.name = name;
                }
            }
        }
    }
    ensure_auto_model(
        by_id
            .into_values()
            .map(|(mut model, _)| {
                if !model.reasoning_levels.is_empty() {
                    model.reasoning_levels =
                        ReasoningLevelSet::new(std::mem::take(&mut model.reasoning_levels))
                            .into_levels();
                }
                model
            })
            .collect(),
    )
}

fn ensure_auto_model(mut models: Vec<CursorModel>) -> Vec<CursorModel> {
    if !models.iter().any(|model| model.id == "auto") {
        models.insert(
            0,
            CursorModel {
                id: "auto".into(),
                name: "Auto".into(),
                reasoning_levels: Vec::new(),
                context_window: DEFAULT_CONTEXT_WINDOW,
                max_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            },
        );
    }
    models
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
