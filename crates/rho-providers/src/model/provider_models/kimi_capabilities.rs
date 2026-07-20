use serde::Deserialize;

use crate::{
    model::{ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

#[derive(Deserialize)]
pub(super) struct KimiReasoningMetadata {
    supports_reasoning: Option<bool>,
    #[serde(rename = "supports_thinking_type")]
    _supports_thinking_type: Option<KimiThinkingType>,
    think_efforts: Option<KimiThinkEfforts>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum KimiThinkingType {
    Only,
    Enabled,
    Disabled,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct KimiThinkEfforts {
    support: Option<bool>,
    valid_efforts: Option<Vec<KimiReasoningEffort>>,
    #[serde(rename = "default_effort")]
    _default_effort: Option<KimiReasoningEffort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum KimiReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    #[serde(other)]
    Other,
}

pub(super) fn reasoning_capabilities(metadata: &KimiReasoningMetadata) -> ReasoningCapabilities {
    match metadata.supports_reasoning {
        Some(false) => {
            return ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Off,
            ]));
        }
        Some(true) => {}
        None => return ReasoningCapabilities::Unknown,
    }
    let Some(think_efforts) = &metadata.think_efforts else {
        return ReasoningCapabilities::Unknown;
    };
    if think_efforts.support != Some(true) {
        return ReasoningCapabilities::Unknown;
    }
    let Some(valid_efforts) = &think_efforts.valid_efforts else {
        return ReasoningCapabilities::Unknown;
    };
    if valid_efforts.is_empty() {
        return ReasoningCapabilities::Unknown;
    }
    let mut levels = Vec::with_capacity(valid_efforts.len() + 1);
    for effort in valid_efforts {
        levels.push(match effort {
            KimiReasoningEffort::None => ReasoningLevel::Off,
            KimiReasoningEffort::Minimal => ReasoningLevel::Minimal,
            KimiReasoningEffort::Low => ReasoningLevel::Low,
            KimiReasoningEffort::Medium => ReasoningLevel::Medium,
            KimiReasoningEffort::High => ReasoningLevel::High,
            KimiReasoningEffort::Xhigh => ReasoningLevel::Xhigh,
            KimiReasoningEffort::Max => ReasoningLevel::Max,
            KimiReasoningEffort::Other => return ReasoningCapabilities::Unknown,
        });
    }
    levels.push(ReasoningLevel::Off);
    ReasoningCapabilities::Levels(ReasoningLevelSet::new(levels))
}

#[cfg(test)]
#[path = "kimi_capabilities_tests.rs"]
mod tests;
