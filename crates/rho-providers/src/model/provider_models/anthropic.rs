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

/// Anthropic `output_config.effort` names, cheapest first.
///
/// Shared by picker derivation and request clamping. The effort protocol has
/// no `minimal`, so that Rho level never appears here.
pub(crate) const EFFORT_NAMES: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

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

    fn adaptive(&self) -> bool {
        leaf_supported(
            self.thinking
                .as_ref()
                .and_then(|block| block.types.adaptive.as_ref()),
        )
    }

    fn enabled(&self) -> bool {
        leaf_supported(
            self.thinking
                .as_ref()
                .and_then(|block| block.types.enabled.as_ref()),
        )
    }

    /// `Some` only when the Models API advertised a `disabled` leaf.
    fn disabled(&self) -> Option<bool> {
        self.thinking.as_ref()?.types.disabled.as_ref()?.supported
    }

    fn effort_supported(&self) -> bool {
        self.effort
            .as_ref()
            .and_then(|block| block.supported)
            .unwrap_or(false)
    }

    fn effort_level_at(&self, index: usize) -> bool {
        let Some(block) = &self.effort else {
            return false;
        };
        match index {
            0 => leaf_supported(block.low.as_ref()),
            1 => leaf_supported(block.medium.as_ref()),
            2 => leaf_supported(block.high.as_ref()),
            3 => leaf_supported(block.xhigh.as_ref()),
            4 => leaf_supported(block.max.as_ref()),
            _ => false,
        }
    }

    /// Single mode used by pickers and the request builder.
    pub(crate) fn thinking_mode(&self, model: &str) -> AnthropicThinkingMode {
        AnthropicThinkingMode::from_capabilities(model, self)
    }

    /// Selectable reasoning levels for pickers and validation.
    pub(crate) fn reasoning_capabilities(&self, model: &str) -> ReasoningCapabilities {
        self.thinking_mode(model).reasoning_capabilities()
    }
}

fn leaf_supported(leaf: Option<&SupportedLeaf>) -> bool {
    leaf.and_then(|leaf| leaf.supported).unwrap_or(false)
}

/// How Off is encoded for a model, given what the Models API advertises.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OffThinking {
    #[default]
    Omit,
    Disabled,
    Unsupported,
}

/// Advertised effort leaves, parallel to [`EFFORT_NAMES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffortMask {
    levels: [bool; EFFORT_NAMES.len()],
}

impl EffortMask {
    fn from_capabilities(capabilities: &AnthropicModelCapabilities) -> Option<Self> {
        if !capabilities.effort_supported() {
            return None;
        }
        let levels = std::array::from_fn(|index| capabilities.effort_level_at(index));
        levels.iter().any(|supported| *supported).then_some(Self { levels })
    }

    fn reasoning_levels(self) -> Vec<ReasoningLevel> {
        EFFORT_NAMES
            .into_iter()
            .enumerate()
            .filter(|(index, _)| self.levels[*index])
            .map(|(_, name)| {
                name.parse()
                    .expect("EFFORT_NAMES entries are valid ReasoningLevel values")
            })
            .collect()
    }

    /// Maps a reasoning level onto the nearest advertised effort, preferring
    /// the cheaper side so an unsupported request never escalates cost. A
    /// request below the advertised range still rises to the model minimum.
    pub(crate) fn for_level(self, reasoning: ReasoningLevel) -> &'static str {
        let requested = match reasoning {
            ReasoningLevel::Off | ReasoningLevel::Minimal | ReasoningLevel::Low => 0,
            ReasoningLevel::Medium => 1,
            ReasoningLevel::High => 2,
            ReasoningLevel::Xhigh => 3,
            ReasoningLevel::Max => 4,
        };
        (0..=requested)
            .rev()
            .chain(requested + 1..EFFORT_NAMES.len())
            .find(|&index| self.levels[index])
            .map(|index| EFFORT_NAMES[index])
            .expect("EffortMask is non-empty")
    }
}

/// Controllable thinking surface derived from Models API capabilities.
///
/// Pickers and the wire path both project from this enum so they cannot drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnthropicThinkingMode {
    /// Adaptive thinking with an effort vocabulary.
    Adaptive { off: OffThinking, efforts: EffortMask },
    /// Legacy budget-token thinking.
    BudgetTokens { off: OffThinking },
    /// Capabilities were fetched but advertise no controllable thinking surface.
    NoControl { off: OffThinking },
}

impl AnthropicThinkingMode {
    pub(crate) fn from_capabilities(
        model: &str,
        capabilities: &AnthropicModelCapabilities,
    ) -> Self {
        let off = off_thinking(model, capabilities.disabled());
        if capabilities.adaptive() {
            if let Some(efforts) = EffortMask::from_capabilities(capabilities) {
                return Self::Adaptive { off, efforts };
            }
            return Self::NoControl { off };
        }
        if capabilities.enabled() {
            return Self::BudgetTokens { off };
        }
        Self::NoControl { off }
    }

    pub(crate) fn off(self) -> OffThinking {
        match self {
            Self::Adaptive { off, .. }
            | Self::BudgetTokens { off }
            | Self::NoControl { off } => off,
        }
    }

    pub(crate) fn reasoning_capabilities(self) -> ReasoningCapabilities {
        let mut levels = match self {
            Self::Adaptive { efforts, .. } => efforts.reasoning_levels(),
            Self::BudgetTokens { .. } => budget_token_levels(),
            Self::NoControl { .. } => {
                return ReasoningCapabilities::NotConfigurable;
            }
        };
        if levels.is_empty() {
            return ReasoningCapabilities::NotConfigurable;
        }
        if self.off() != OffThinking::Unsupported {
            levels.push(ReasoningLevel::Off);
        }
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(levels))
    }
}

/// Resolves the Off encoding from the advertised `disabled` leaf, falling back
/// to per-family defaults when the Models API omits it.
fn off_thinking(model: &str, disabled_leaf: Option<bool>) -> OffThinking {
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

/// Budget-token thinking accepts any positive Rho level; Off is handled apart.
fn budget_token_levels() -> Vec<ReasoningLevel> {
    ReasoningLevel::ALL
        .into_iter()
        .filter(|level| *level != ReasoningLevel::Off)
        .collect()
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

/// Resolves the cached thinking mode, falling back to the parent alias for
/// dated snapshot ids. `None` means no cached capabilities row.
pub(crate) fn cached_thinking_mode(model: &str) -> Option<AnthropicThinkingMode> {
    cached_capabilities_for_id(model)
        .or_else(|| dated_parent_model(model).and_then(cached_capabilities_for_id))
        .map(|capabilities| capabilities.thinking_mode(model))
}

#[cfg(test)]
pub(crate) fn thinking_mode_from_value(
    model: &str,
    value: &Value,
) -> Option<AnthropicThinkingMode> {
    AnthropicModelCapabilities::from_value(value).map(|capabilities| capabilities.thinking_mode(model))
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
                .map(|capabilities| capabilities.reasoning_capabilities(&model.id))
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
