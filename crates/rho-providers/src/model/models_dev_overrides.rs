//! TOML model overrides parsing for models.dev catalog entries.
//!
//! Handles built-in `model_overrides.toml` and user-configured local `models.toml`.

use std::{fs, path::PathBuf, sync::OnceLock};

use crate::reasoning::ReasoningLevel;

use super::{ModelCost, ModelMetadata};

const BUILTIN_MODEL_OVERRIDES_TOML: &str = include_str!("model_overrides.toml");

pub(super) fn apply_builtin_overrides(
    provider: &str,
    model: &str,
    metadata: ModelMetadata,
) -> ModelMetadata {
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

pub(super) fn apply_local_overrides(
    provider: &str,
    model: &str,
    metadata: ModelMetadata,
) -> ModelMetadata {
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

pub(super) fn merge_toml_override(
    mut metadata: ModelMetadata,
    table: &toml::map::Map<String, toml::Value>,
) -> ModelMetadata {
    metadata.display_name = table
        .get("display_name")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or(metadata.display_name);
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
