use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    credentials::CredentialStore,
    model::{ModelError, ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

use super::{load_api_key_auth, provider_models_client, ProviderModel};

/// Upper bound on `/v1/models` pages so a misbehaving cursor cannot hang the
/// startup refresh.
const MAX_MODEL_PAGES: usize = 20;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct AnthropicModelCapabilities {
    #[serde(default)]
    thinking: Option<ThinkingBlock>,
    #[serde(default)]
    effort: Option<EffortBlock>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
struct ThinkingBlock {
    #[serde(default)]
    types: ThinkingTypes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
struct ThinkingTypes {
    #[serde(default)]
    adaptive: Option<SupportedLeaf>,
    #[serde(default)]
    enabled: Option<SupportedLeaf>,
    #[serde(default)]
    disabled: Option<SupportedLeaf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
struct EffortBlock {
    #[serde(default)]
    supported: Option<bool>,
    #[serde(default)]
    low: Option<SupportedLeaf>,
    #[serde(default)]
    medium: Option<SupportedLeaf>,
    #[serde(default)]
    high: Option<SupportedLeaf>,
    #[serde(default)]
    xhigh: Option<SupportedLeaf>,
    #[serde(default)]
    max: Option<SupportedLeaf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
struct SupportedLeaf {
    #[serde(default)]
    supported: Option<bool>,
}

impl AnthropicModelCapabilities {
    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        value
            .is_object()
            .then(|| serde_json::from_value(value.clone()).ok())
            .flatten()
    }

    pub(crate) fn adaptive(&self) -> bool {
        leaf_supported(
            self.thinking
                .as_ref()
                .and_then(|block| block.types.adaptive.as_ref()),
        )
    }

    pub(crate) fn enabled(&self) -> bool {
        leaf_supported(
            self.thinking
                .as_ref()
                .and_then(|block| block.types.enabled.as_ref()),
        )
    }

    /// `Some` only when the Models API advertised a `disabled` leaf.
    pub(crate) fn disabled(&self) -> Option<bool> {
        self.thinking.as_ref()?.types.disabled.as_ref()?.supported
    }

    pub(crate) fn effort_supported(&self) -> bool {
        self.effort
            .as_ref()
            .and_then(|block| block.supported)
            .unwrap_or(false)
    }

    pub(crate) fn effort_level(&self, level: &str) -> bool {
        let Some(block) = &self.effort else {
            return false;
        };
        match level {
            "low" => leaf_supported(block.low.as_ref()),
            "medium" => leaf_supported(block.medium.as_ref()),
            "high" => leaf_supported(block.high.as_ref()),
            "xhigh" => leaf_supported(block.xhigh.as_ref()),
            "max" => leaf_supported(block.max.as_ref()),
            _ => false,
        }
    }
}

fn leaf_supported(leaf: Option<&SupportedLeaf>) -> bool {
    leaf.and_then(|leaf| leaf.supported).unwrap_or(false)
}

/// Effort levels Anthropic's `output_config` accepts, cheapest first. The
/// protocol has no `minimal`, so no reasoning level maps onto one.
pub(crate) const EFFORT_LEVELS: [(&str, ReasoningLevel); 5] = [
    ("low", ReasoningLevel::Low),
    ("medium", ReasoningLevel::Medium),
    ("high", ReasoningLevel::High),
    ("xhigh", ReasoningLevel::Xhigh),
    ("max", ReasoningLevel::Max),
];

/// How Off is encoded for a model, given what the Models API advertises.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OffThinking {
    #[default]
    Omit,
    Disabled,
    Unsupported,
}

/// Resolves the Off encoding from the advertised `disabled` leaf, falling back
/// to per-family defaults when the Models API omits it.
pub(crate) fn off_thinking(model: &str, disabled_leaf: Option<bool>) -> OffThinking {
    match disabled_leaf {
        Some(true) => OffThinking::Disabled,
        Some(false) => OffThinking::Unsupported,
        None => off_when_unadvertised(model),
    }
}

/// Models API has no `disabled` leaf on current Claude 5 rows. These prefixes
/// fill that gap; a present leaf always wins.
fn off_when_unadvertised(model: &str) -> OffThinking {
    if model_has_prefix(model, &["claude-opus-5", "claude-sonnet-5"]) {
        OffThinking::Disabled
    } else if model_has_prefix(
        model,
        &["claude-fable-5", "claude-mythos-5", "claude-mythos-preview"],
    ) {
        OffThinking::Unsupported
    } else {
        OffThinking::Omit
    }
}

fn model_has_prefix(model: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        model == *prefix
            || model
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('-'))
    })
}

/// Derives selectable reasoning levels from the Models API capabilities, so
/// pickers offer exactly what the wire protocol accepts.
///
/// Adaptive models take their levels from the advertised effort vocabulary,
/// which never includes `minimal`. Budget-token models accept any budget down
/// to Anthropic's 1024 minimum, so the whole ladder is selectable. Anything
/// else stays `Unknown` rather than inventing a control.
pub(crate) fn reasoning_capabilities(
    model: &str,
    capabilities: &AnthropicModelCapabilities,
) -> ReasoningCapabilities {
    let mut levels = if capabilities.adaptive() {
        if !capabilities.effort_supported() {
            return ReasoningCapabilities::Unknown;
        }
        EFFORT_LEVELS
            .iter()
            .filter(|(name, _)| capabilities.effort_level(name))
            .map(|(_, level)| *level)
            .collect::<Vec<_>>()
    } else if capabilities.enabled() {
        vec![
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
            ReasoningLevel::Max,
        ]
    } else {
        return ReasoningCapabilities::Unknown;
    };
    if levels.is_empty() {
        return ReasoningCapabilities::Unknown;
    }
    if off_thinking(model, capabilities.disabled()) != OffThinking::Unsupported {
        levels.push(ReasoningLevel::Off);
    }
    ReasoningCapabilities::Levels(ReasoningLevelSet::new(levels))
}

/// Strips a trailing `-YYYYMMDD` snapshot suffix, yielding the parent alias
/// that carries the cached capabilities row.
pub(super) fn dated_parent_model(model: &str) -> Option<&str> {
    let (parent, date) = model.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit())).then_some(parent)
}

pub(super) fn capabilities_json_is_known(raw_json: Option<&str>) -> bool {
    raw_json
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .as_ref()
        .and_then(AnthropicModelCapabilities::from_value)
        .is_some()
}

/// Resolves cached Models API capabilities, falling back to the parent alias
/// for dated snapshot ids.
pub(crate) fn cached_capabilities(model: &str) -> Option<AnthropicModelCapabilities> {
    cached_capabilities_for_id(model)
        .or_else(|| dated_parent_model(model).and_then(cached_capabilities_for_id))
}

fn cached_capabilities_for_id(model: &str) -> Option<AnthropicModelCapabilities> {
    let value = super::cached_provider_model_raw_json("anthropic", model)?;
    AnthropicModelCapabilities::from_value(&value)
}

#[derive(Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
    max_input_tokens: Option<u64>,
    max_tokens: Option<u64>,
    #[serde(default)]
    capabilities: Option<Value>,
}

enum ModelListContinuation {
    Done,
    Next { after_id: String },
}

fn model_list_continuation(
    has_more: bool,
    last_id: Option<String>,
    after_id: Option<&str>,
) -> ModelListContinuation {
    if !has_more {
        return ModelListContinuation::Done;
    }
    let Some(next_after_id) = last_id else {
        return ModelListContinuation::Done;
    };
    if after_id == Some(next_after_id.as_str()) {
        // The cursor did not advance; stop instead of refetching the page.
        return ModelListContinuation::Done;
    }
    ModelListContinuation::Next {
        after_id: next_after_id,
    }
}

fn model_list_truncated(max_pages: usize) -> ModelError {
    ModelError::InvalidResponse(format!(
        "Anthropic /v1/models exceeded {max_pages} pages while more results remain"
    ))
}

fn capabilities_json(capabilities: Option<Value>) -> Value {
    capabilities
        .filter(|value| AnthropicModelCapabilities::from_value(value).is_some())
        .unwrap_or(Value::Null)
}

fn records_from_page(
    provider: &str,
    response: AnthropicModelsResponse,
) -> Vec<super::ProviderModelRecord> {
    response
        .data
        .into_iter()
        .filter(|model| model.id.starts_with("claude-"))
        .map(|model| {
            let raw_json = capabilities_json(model.capabilities);
            let reasoning_capabilities = AnthropicModelCapabilities::from_value(&raw_json)
                .map(|capabilities| reasoning_capabilities(&model.id, &capabilities))
                .unwrap_or(ReasoningCapabilities::Unknown);
            super::ProviderModelRecord {
                model: ProviderModel {
                    provider: provider.to_string(),
                    display_name: model.display_name.unwrap_or_else(|| model.id.clone()),
                    context_window: model.max_input_tokens.filter(|window| *window > 0),
                    max_output_tokens: model.max_tokens,
                    model: model.id,
                    reasoning_capabilities,
                },
                raw_json,
            }
        })
        .collect()
}

fn add_page(
    models: &mut Vec<super::ProviderModelRecord>,
    provider: &str,
    response: AnthropicModelsResponse,
    after_id: Option<&str>,
) -> ModelListContinuation {
    let has_more = response.has_more;
    let last_id = response.last_id.clone();
    models.extend(records_from_page(provider, response));
    model_list_continuation(has_more, last_id, after_id)
}

fn finalize_models(mut models: Vec<super::ProviderModelRecord>) -> Vec<super::ProviderModelRecord> {
    models.sort_by(|left, right| left.model.model.cmp(&right.model.model));
    models.dedup_by(|left, right| left.model.model == right.model.model);
    models
}

pub(super) async fn fetch(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<super::ProviderModelRecord>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
    let client = provider_models_client()?;
    let mut models = Vec::new();
    let mut after_id = None::<String>;
    let base = Url::parse("https://api.anthropic.com/v1/models").map_err(|err| {
        ModelError::InvalidResponse(format!("invalid Anthropic models URL: {err}"))
    })?;
    for _ in 0..MAX_MODEL_PAGES {
        let mut url = base.clone();
        if let Some(after_id) = &after_id {
            url.query_pairs_mut().append_pair("after_id", after_id);
        }
        let response: AnthropicModelsResponse = client
            .get(url)
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match add_page(&mut models, provider, response, after_id.as_deref()) {
            ModelListContinuation::Done => return Ok(finalize_models(models)),
            ModelListContinuation::Next {
                after_id: next_after_id,
            } => after_id = Some(next_after_id),
        }
    }
    Err(model_list_truncated(MAX_MODEL_PAGES))
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
