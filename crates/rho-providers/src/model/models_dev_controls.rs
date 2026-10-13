//! Catalog controls kept alongside the public metadata until its next major.

use serde::{Deserialize, Serialize};

use crate::model::{provider_models, ReasoningCapabilities};

use super::{
    load_cached_row, override_metadata, overrides, CacheFreshness, ImageInput, ModelMetadata,
};

/// Whether the catalog's reasoning levels represent named effort or a binary toggle.
// NEXT_MAJOR(rho-providers): add reasoning_control to ModelMetadata and remove the sidecar lookup and cache field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReasoningControl {
    #[default]
    Effort,
    Toggle,
}

/// Resolve current catalog provenance with the same key and overrides as metadata.
pub(crate) fn current_reasoning_metadata(
    provider: &str,
    model: &str,
) -> (Option<ModelMetadata>, ReasoningControl) {
    match load_cached_row(provider, model, CacheFreshness::CurrentOnly) {
        Some(row) => (Some(row.metadata), row.reasoning_control),
        None => (override_metadata(provider, model), ReasoningControl::Effort),
    }
}

// NEXT_MAJOR(rho-providers): add `image_input: ImageInput` to ModelMetadata and remove the separate models_dev::image_input lookup and its sidecar cache key.
/// One cache row, flattening minor-compatible sidecars beside public metadata.
#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct CachedRow {
    #[serde(flatten)]
    pub(super) metadata: ModelMetadata,
    #[serde(default)]
    pub(super) image_input: ImageInput,
    #[serde(default)]
    pub(super) reasoning_control: ReasoningControl,
}

impl CachedRow {
    /// Apply a model override table; explicit levels replace catalog toggle semantics.
    pub(super) fn with_toml_overrides(
        mut self,
        local: Option<&toml::map::Map<String, toml::Value>>,
    ) -> Self {
        if let Some(table) = local {
            if overrides::toml_reasoning_levels(table, "supported_reasoning_levels").is_some() {
                self.reasoning_control = ReasoningControl::Effort;
            }
            self.image_input = overrides::local_image_input(table).unwrap_or(self.image_input);
            self.metadata = overrides::merge_toml_override(self.metadata, table);
        }
        self
    }

    /// Provider-advertised levels take precedence over catalog toggle semantics.
    pub(super) fn with_provider_capabilities(mut self, provider: &str, model: &str) -> Self {
        let provider_model = provider_models::cached_provider_model(provider, model);
        let context_window = provider_model
            .as_ref()
            .and_then(|model| model.context_window)
            .or_else(|| {
                crate::provider::provider_descriptor(provider)
                    .and_then(|descriptor| descriptor.effective_context_fallback(model))
            });
        if let Some(context_window) = context_window {
            self.metadata.effective_context_window = Some(context_window);
        }
        if let Some(provider_model) = provider_model
            .filter(|_| !provider_models::provider_model_capabilities_need_refresh(provider, model))
        {
            match provider_model.reasoning_capabilities {
                ReasoningCapabilities::Unknown => {}
                ReasoningCapabilities::NotConfigurable => {
                    self.metadata.supported_reasoning_levels = None;
                    self.metadata.reasoning_capabilities_known = true;
                    self.metadata.reasoning_metadata_complete = true;
                    self.reasoning_control = ReasoningControl::Effort;
                }
                ReasoningCapabilities::Levels(levels) => {
                    self.metadata.supported_reasoning_levels = Some(levels.into_levels());
                    self.metadata.reasoning_capabilities_known = true;
                    self.metadata.reasoning_metadata_complete = true;
                    self.reasoning_control = ReasoningControl::Effort;
                }
            }
        }
        self
    }
}

#[cfg(test)]
#[path = "models_dev_controls_tests.rs"]
mod tests;
