use crate::{
    model::provider_models::{self, AnthropicModelCapabilities},
    protocol::anthropic_messages::{AnthropicOutputConfig, AnthropicThinkingConfig},
    provider_backend::ModelError,
    reasoning::ReasoningLevel,
};

use super::ANTHROPIC_ANSWER_RESERVE_TOKENS;

/// How Off is encoded when a catalog advertises that choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OffThinking {
    #[default]
    Omit,
    Disabled,
    Unsupported,
}

/// Wire protocol advertised by Anthropic's Models API `capabilities` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AnthropicThinkingProtocol {
    model: String,
    adaptive: bool,
    enabled: bool,
    off: OffThinking,
    effort: EffortSupport,
}

/// Effort levels the model advertises, cheapest first, indexed like
/// `EFFORT_LEVELS`.
const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EffortSupport {
    supported: bool,
    levels: [bool; EFFORT_LEVELS.len()],
}

impl AnthropicThinkingProtocol {
    #[cfg(test)]
    pub(crate) fn from_capabilities(model: &str, capabilities: &serde_json::Value) -> Self {
        match AnthropicModelCapabilities::from_value(capabilities) {
            Some(parsed) => Self::from_parsed(model, &parsed),
            None => Self::unknown(model),
        }
    }

    fn from_parsed(model: &str, capabilities: &AnthropicModelCapabilities) -> Self {
        Self {
            model: model.to_string(),
            adaptive: capabilities.adaptive(),
            enabled: capabilities.enabled(),
            off: off_thinking(model, capabilities.disabled()),
            effort: EffortSupport {
                supported: capabilities.effort_supported(),
                levels: EFFORT_LEVELS.map(|level| capabilities.effort_level(level)),
            },
        }
    }

    fn unknown(model: &str) -> Self {
        Self {
            model: model.to_string(),
            ..Self::default()
        }
    }
}

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

/// Resolves the cached Models API capabilities for `model`, including the
/// parent alias for dated snapshot ids. Resolved once at provider construction;
/// request building stays pure.
pub(super) fn resolve_thinking_protocol(model: &str) -> AnthropicThinkingProtocol {
    match provider_models::cached_anthropic_capabilities(model) {
        Some(capabilities) => AnthropicThinkingProtocol::from_parsed(model, &capabilities),
        None => AnthropicThinkingProtocol::unknown(model),
    }
}

pub(super) fn thinking_config_for(
    protocol: &AnthropicThinkingProtocol,
    reasoning: ReasoningLevel,
    max_tokens: u32,
) -> Result<
    (
        Option<AnthropicThinkingConfig>,
        Option<AnthropicOutputConfig>,
    ),
    ModelError,
> {
    if reasoning == ReasoningLevel::Off {
        // Off never sends output_config, so xhigh/max cannot ride along.
        return match protocol.off {
            OffThinking::Unsupported => Err(ModelError::UnsupportedReasoning {
                provider: "anthropic",
                model: protocol.model.clone(),
                requested: reasoning,
            }),
            OffThinking::Disabled => Ok((Some(AnthropicThinkingConfig::Disabled), None)),
            OffThinking::Omit => Ok((None, None)),
        };
    }
    if protocol.adaptive {
        return Ok((
            Some(AnthropicThinkingConfig::Adaptive {
                display: "summarized",
            }),
            protocol
                .effort
                .for_level(reasoning)
                .map(|effort| AnthropicOutputConfig { effort }),
        ));
    }
    if !protocol.enabled {
        // Unknown models omit thinking. Sending `enabled` 400s on current
        // Claude 5 / 4.7+ models; omitting leaves thinking at the model default.
        return Ok((None, None));
    }

    let requested_budget = match reasoning {
        ReasoningLevel::Off => unreachable!("Off is handled before budget selection"),
        ReasoningLevel::Minimal => 1_024,
        ReasoningLevel::Low => 2_048,
        ReasoningLevel::Medium => 4_096,
        ReasoningLevel::High => 8_192,
        ReasoningLevel::Xhigh => 16_384,
        ReasoningLevel::Max => 32_768,
    };
    let available = max_tokens.saturating_sub(ANTHROPIC_ANSWER_RESERVE_TOKENS);
    if available < 1_024 {
        return Err(ModelError::InvalidResponse(format!(
            "Anthropic max output tokens {max_tokens} cannot reserve a reasoning budget"
        )));
    }
    Ok((
        Some(AnthropicThinkingConfig::Enabled {
            budget_tokens: requested_budget.min(available),
        }),
        None,
    ))
}

impl EffortSupport {
    /// Maps a reasoning level onto the nearest advertised effort, preferring
    /// the cheaper side so an unsupported request never escalates cost. A
    /// request below the advertised range still rises to the model minimum.
    fn for_level(self, reasoning: ReasoningLevel) -> Option<&'static str> {
        if !self.supported {
            return None;
        }
        let requested = match reasoning {
            ReasoningLevel::Off | ReasoningLevel::Minimal | ReasoningLevel::Low => 0,
            ReasoningLevel::Medium => 1,
            ReasoningLevel::High => 2,
            ReasoningLevel::Xhigh => 3,
            ReasoningLevel::Max => 4,
        };
        (0..=requested)
            .rev()
            .chain(requested + 1..EFFORT_LEVELS.len())
            .find(|&index| self.levels[index])
            .map(|index| EFFORT_LEVELS[index])
    }
}

#[cfg(test)]
#[path = "thinking_tests.rs"]
mod tests;
