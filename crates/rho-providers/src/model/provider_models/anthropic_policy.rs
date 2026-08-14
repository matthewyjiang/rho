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
/// Incomplete shapes stay [`Self::Unknown`] (or adaptive without an effort
/// mask) rather than inventing a non-configurable contract from missing data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnthropicThinkingMode {
    /// Adaptive thinking. `efforts` is set only when the API advertised a
    /// usable effort vocabulary; otherwise the wire sends adaptive without
    /// clamping and the picker keeps the unknown fallback.
    Adaptive {
        off: OffThinking,
        efforts: Option<EffortMask>,
    },
    /// Legacy budget-token thinking.
    BudgetTokens { off: OffThinking },
    /// Capabilities were fetched but do not identify adaptive or budget control.
    /// Distinct from a missing cache row: Off still follows family rules, and
    /// non-Off leaves the model default rather than inventing a protocol.
    Unknown { off: OffThinking },
}

impl AnthropicThinkingMode {
    /// Hosted Messages adapters have no Anthropic Models API row. Map the
    /// host catalog's advertised levels onto the legacy budget-token surface
    /// so a picker level can be encoded instead of failing as unsupported.
    ///
    /// Generic catalog effort levels do not prove the model supports the
    /// `thinking.type: "adaptive"` protocol, so adaptive is never inferred
    /// here. Adaptive selection requires an explicit signal from the host or a
    /// model-specific protocol contract (the first-party Models API path).
    pub(crate) fn from_host_catalog_capabilities(capabilities: ReasoningCapabilities) -> Self {
        match capabilities {
            ReasoningCapabilities::NotConfigurable => Self::Unknown {
                off: OffThinking::Omit,
            },
            ReasoningCapabilities::Unknown => Self::BudgetTokens {
                off: OffThinking::Disabled,
            },
            ReasoningCapabilities::Levels(levels) => {
                let advertised = levels.levels();
                let off = if advertised.contains(&ReasoningLevel::Off) {
                    OffThinking::Disabled
                } else {
                    OffThinking::Unsupported
                };
                Self::BudgetTokens { off }
            }
        }
    }

    pub(crate) fn from_capabilities(
        model: &str,
        capabilities: &AnthropicModelCapabilities,
    ) -> Self {
        let off = off_thinking(model, capabilities.disabled());
        if capabilities.adaptive() {
            return Self::Adaptive {
                off,
                efforts: EffortMask::from_capabilities(capabilities),
            };
        }
        if capabilities.enabled() {
            return Self::BudgetTokens { off };
        }
        Self::Unknown { off }
    }

    pub(crate) fn off(self) -> OffThinking {
        match self {
            Self::Adaptive { off, .. } | Self::BudgetTokens { off } | Self::Unknown { off } => off,
        }
    }

    pub(crate) fn reasoning_capabilities(self) -> ReasoningCapabilities {
        let mut levels = match self {
            Self::Adaptive {
                efforts: Some(efforts),
                ..
            } => efforts.reasoning_levels(),
            Self::BudgetTokens { .. } => budget_token_levels(),
            // Absence of a complete control is not proof the model has none.
            Self::Adaptive { efforts: None, .. } | Self::Unknown { .. } => {
                return ReasoningCapabilities::Unknown;
            }
        };
        if levels.is_empty() {
            return ReasoningCapabilities::Unknown;
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
/// Omitted or non-object values become `{}` so a successful fetch is recorded
/// as a known row (no perpetual refresh). Projection of that empty object is
/// still [`AnthropicThinkingMode::Unknown`]: known cache identity is not the
/// same as a proven non-configurable thinking surface.
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
