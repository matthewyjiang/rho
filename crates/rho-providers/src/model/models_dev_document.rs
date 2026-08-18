//! Typed models.dev snapshot.
//!
//! The live catalog is several megabytes of JSON. Deserializing that document as
//! `serde_json::Value` builds a 20-35 MB tree because every unused field stays
//! on the heap. This module keeps only the provider and model fields Rho reads.

#[path = "models_dev_document_de.rs"]
mod de;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;

use crate::{provider::CatalogReasoningPolicy, reasoning::ReasoningLevel};

use super::{ModelCost, ModelMetadata, ReasoningOffBehavior};

/// models.dev root object: provider id -> provider document.
#[derive(Clone, Debug, Default)]
pub(super) struct ModelsDevCatalog {
    providers: HashMap<String, ModelsDevProvider>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ModelsDevProvider {
    pub npm: Option<String>,
    pub models: HashMap<String, ModelsDevModel>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct ModelsDevModel {
    #[serde(deserialize_with = "de::lenient_or_default")]
    name: Option<String>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    status: Option<String>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    npm: Option<String>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    pub provider: ModelsDevModelProvider,
    #[serde(deserialize_with = "de::lenient_or_default")]
    limit: ModelsDevLimit,
    #[serde(deserialize_with = "de::lenient_or_default")]
    cost: Option<ModelsDevCost>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    reasoning: Option<bool>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    reasoning_options: Option<CatalogReasoningOptions>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct ModelsDevModelProvider {
    #[serde(deserialize_with = "de::lenient_or_default")]
    pub npm: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ModelsDevLimit {
    #[serde(deserialize_with = "de::lenient_or_default")]
    context: Option<u64>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    input: Option<u64>,
    #[serde(deserialize_with = "de::lenient_or_default")]
    output: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct ModelsDevCost {
    default: Option<ModelCost>,
    long_context_threshold: Option<u64>,
    long_context: Option<ModelCost>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ModelsDevCostRates {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ModelsDevCostTier {
    size: Option<u64>,
    rates: ModelsDevCostRates,
}

#[derive(Clone, Debug, Default)]
struct CatalogReasoningOptions {
    empty: bool,
    has_toggle: bool,
    has_effort: bool,
    effort_values: Option<Vec<Option<String>>>,
}

impl ModelsDevCatalog {
    pub(super) fn provider(&self, name: &str) -> Option<&ModelsDevProvider> {
        self.providers.get(name)
    }

    pub(super) fn iter_providers(&self) -> impl Iterator<Item = (&str, &ModelsDevProvider)> {
        self.providers
            .iter()
            .map(|(name, provider)| (name.as_str(), provider))
    }

    pub(super) fn model(
        &self,
        provider: &str,
        model: &str,
    ) -> Option<(&ModelsDevProvider, &ModelsDevModel)> {
        let provider_doc = self.provider(provider)?;
        let model_doc = provider_doc.models.get(model).or_else(|| {
            model
                .strip_prefix("openai/")
                .and_then(|id| provider_doc.models.get(id))
                .or_else(|| provider_doc.models.get(&format!("{provider}/{model}")))
        })?;
        Some((provider_doc, model_doc))
    }

    #[cfg(test)]
    pub(super) fn from_json_value(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).expect("models.dev test fixture must deserialize")
    }
}

pub(super) fn model_metadata_from_catalog(
    api: &ModelsDevCatalog,
    provider: &str,
    model: &str,
    reasoning_policy: CatalogReasoningPolicy,
) -> Option<ModelMetadata> {
    let (provider_doc, model) = api.model(provider, model)?;
    Some(ModelMetadata {
        display_name: model
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        advertised_context_window: model.limit.context,
        effective_context_window: model.limit.input.or(model.limit.context),
        usable_context_window: None,
        long_context_threshold: model
            .cost
            .as_ref()
            .and_then(|cost| cost.long_context_threshold),
        max_output_tokens: model.limit.output,
        cost_default: model.cost.as_ref().and_then(|cost| cost.default),
        cost_long_context: model.cost.as_ref().and_then(|cost| cost.long_context),
        supported_reasoning_levels: supported_reasoning_levels(model, reasoning_policy),
        reasoning_off_behavior: if advertised_none_effort(model) {
            ReasoningOffBehavior::EffortNone
        } else {
            ReasoningOffBehavior::Omit
        },
        reasoning_capabilities_known: reasoning_capabilities_known(model, reasoning_policy),
        reasoning_metadata_complete: reasoning_metadata_complete(model, reasoning_policy),
        sdk_package: resolved_sdk_package(provider_doc, model),
    })
}

/// models.dev provider `npm`, overridden by per-model `provider.npm` or `npm`.
fn resolved_sdk_package(provider: &ModelsDevProvider, model: &ModelsDevModel) -> Option<String> {
    model
        .provider
        .npm
        .as_deref()
        .or(model.npm.as_deref())
        .or(provider.npm.as_deref())
        .map(str::trim)
        .filter(|package| !package.is_empty())
        .map(str::to_string)
}

pub(super) fn deprecated_provider_models(
    api: &ModelsDevCatalog,
    provider: &str,
) -> HashSet<String> {
    api.provider(provider)
        .into_iter()
        .flat_map(|provider| {
            provider
                .models
                .iter()
                .filter(|(_, model)| model.status.as_deref() == Some("deprecated"))
                .map(|(id, _)| id.clone())
        })
        .collect()
}

fn reasoning_metadata_complete(model: &ModelsDevModel, policy: CatalogReasoningPolicy) -> bool {
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

fn reasoning_capabilities_known(model: &ModelsDevModel, policy: CatalogReasoningPolicy) -> bool {
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
    let Some(supports_reasoning) = model.reasoning else {
        // Missing capability signal: keep the row incomplete so a fresher
        // models.dev snapshot can still be fetched.
        return false;
    };
    if !supports_reasoning {
        return true;
    }
    let Some(options) = &model.reasoning_options else {
        return false;
    };
    if options.empty {
        return true;
    }
    if effort_values(model)
        .is_some_and(|values| !values.is_empty() && values.iter().all(is_recognized_effort_value))
    {
        return true;
    }
    toggle_is_complete_binary_control(model, policy)
}

fn toggle_is_complete_binary_control(
    model: &ModelsDevModel,
    policy: CatalogReasoningPolicy,
) -> bool {
    advertised_toggle(model)
        && matches!(
            policy,
            CatalogReasoningPolicy::OffAsNone | CatalogReasoningPolicy::ExactAdvertised
        )
}

fn is_recognized_effort_value(value: &Option<String>) -> bool {
    matches!(
        value.as_deref(),
        Some("none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max")
    )
}

fn advertised_toggle(model: &ModelsDevModel) -> bool {
    model
        .reasoning_options
        .as_ref()
        .is_some_and(|options| options.has_toggle)
}

fn advertised_none_effort(model: &ModelsDevModel) -> bool {
    effort_values(model)
        .is_some_and(|values| values.iter().any(|value| value.as_deref() == Some("none")))
}

fn effort_values(model: &ModelsDevModel) -> Option<&[Option<String>]> {
    model.reasoning_options.as_ref()?.effort_values.as_deref()
}

fn supported_reasoning_levels(
    model: &ModelsDevModel,
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
    let supports_reasoning = model.reasoning?;
    if !supports_reasoning {
        return None;
    }
    if model
        .reasoning_options
        .as_ref()
        .is_some_and(|options| options.empty)
    {
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
        .filter_map(|value| match value.as_deref()? {
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

impl ModelsDevCost {
    pub(super) fn from_parts(
        rates: ModelsDevCostRates,
        tiers: Vec<ModelsDevCostTier>,
        context_over: BTreeMap<String, ModelsDevCostRates>,
    ) -> Self {
        let (long_context_threshold, long_context) = long_context_cost(&tiers, &context_over);
        Self {
            default: model_cost(rates),
            long_context_threshold,
            long_context,
        }
    }
}

fn model_cost(rates: ModelsDevCostRates) -> Option<ModelCost> {
    let model_cost = ModelCost {
        input_micros_per_m: rates.input.and_then(cost_micros_per_million),
        output_micros_per_m: rates.output.and_then(cost_micros_per_million),
        cache_read_micros_per_m: rates.cache_read.and_then(cost_micros_per_million),
        cache_write_micros_per_m: rates.cache_write.and_then(cost_micros_per_million),
    };
    model_cost_has_rates(&model_cost).then_some(model_cost)
}

fn long_context_cost(
    tiers: &[ModelsDevCostTier],
    context_over: &BTreeMap<String, ModelsDevCostRates>,
) -> (Option<u64>, Option<ModelCost>) {
    for tier in tiers {
        let Some(threshold) = tier.size else {
            continue;
        };
        let Some(model_cost) = model_cost(tier.rates) else {
            continue;
        };
        return (Some(threshold), Some(model_cost));
    }

    for (key, rates) in context_over {
        let Some(threshold) = context_over_threshold(key) else {
            continue;
        };
        let Some(model_cost) = model_cost(*rates) else {
            continue;
        };
        return (Some(threshold), Some(model_cost));
    }

    (None, None)
}

pub(super) fn context_over_threshold(key: &str) -> Option<u64> {
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

fn cost_micros_per_million(dollars: f64) -> Option<u64> {
    dollars
        .is_finite()
        .then(|| (dollars.max(0.0) * 1_000_000.0).round() as u64)
}
