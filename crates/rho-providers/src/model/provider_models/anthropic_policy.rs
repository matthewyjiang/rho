use serde::Deserialize;
use serde_json::Value;

use crate::{
    model::{ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

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
        levels
            .iter()
            .any(|supported| *supported)
            .then_some(Self { levels })
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

/// Temporary Models API gap shim for Off encoding when the `disabled` leaf is
/// absent. A present leaf always wins.
///
/// Remove this table once `/v1/models` advertises `thinking.types.disabled` on
/// every Claude row that needs a non-default Off encoding. Until then, only
/// add prefixes that product has verified against live traffic — do not grow
/// this into a second capability registry.
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
pub(crate) fn dated_parent_model(model: &str) -> Option<&str> {
    let (parent, date) = model.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit())).then_some(parent)
}

/// True when a cached `raw_json` cell is a capabilities object written by a
/// successful Models API refresh (including the empty object used when the API
/// omitted capabilities).
pub(crate) fn capabilities_json_is_known(raw_json: Option<&str>) -> bool {
    raw_json
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .as_ref()
        .and_then(AnthropicModelCapabilities::from_value)
        .is_some()
}

/// Normalizes a Models API capabilities field for cache storage.
///
/// Omitted or non-object values become `{}` so a successful fetch is always
/// recorded as known [`AnthropicThinkingMode::NoControl`] rather than cold
/// cache / perpetual refresh.
pub(crate) fn capabilities_json(capabilities: Option<Value>) -> Value {
    capabilities
        .filter(|value| AnthropicModelCapabilities::from_value(value).is_some())
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// Resolves the cached thinking mode, falling back to the parent alias for
/// dated snapshot ids. `None` means no cached capabilities row.
pub(crate) fn cached_thinking_mode(model: &str) -> Option<AnthropicThinkingMode> {
    resolve_cached_capabilities(model).map(|capabilities| capabilities.thinking_mode(model))
}

/// Single cache identity for Anthropic capabilities: exact id, then dated parent.
pub(crate) fn resolve_cached_capabilities(model: &str) -> Option<AnthropicModelCapabilities> {
    cached_capabilities_for_id(model)
        .or_else(|| dated_parent_model(model).and_then(cached_capabilities_for_id))
}

#[cfg(test)]
pub(crate) fn thinking_mode_from_value(
    model: &str,
    value: &Value,
) -> Option<AnthropicThinkingMode> {
    AnthropicModelCapabilities::from_value(value)
        .map(|capabilities| capabilities.thinking_mode(model))
}

fn cached_capabilities_for_id(model: &str) -> Option<AnthropicModelCapabilities> {
    let value = super::super::cached_provider_model_raw_json("anthropic", model)?;
    AnthropicModelCapabilities::from_value(&value)
}

#[cfg(test)]
#[path = "anthropic_policy_tests.rs"]
mod tests;
