use crate::{
    model::provider_models::{self, AnthropicModelCapabilities, OffThinking},
    protocol::anthropic_messages::{AnthropicOutputConfig, AnthropicThinkingConfig},
    provider_backend::ModelError,
    reasoning::ReasoningLevel,
};

use super::ANTHROPIC_ANSWER_RESERVE_TOKENS;

/// Wire protocol advertised by Anthropic's Models API `capabilities` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AnthropicThinkingProtocol {
    model: String,
    /// False when no cached Models API row exists. That is not the same as a
    /// fetched object that advertises no thinking types.
    resolved: bool,
    adaptive: bool,
    enabled: bool,
    off: OffThinking,
    effort: EffortSupport,
}

/// Effort levels the model advertises, cheapest first, indexed like
/// `provider_models::ANTHROPIC_EFFORT_LEVELS`.
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
            resolved: true,
            adaptive: capabilities.adaptive(),
            enabled: capabilities.enabled(),
            off: provider_models::anthropic_off_thinking(model, capabilities.disabled()),
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
    if !protocol.resolved {
        if reasoning == ReasoningLevel::Off {
            return Ok((None, None));
        }
        return Err(ModelError::InvalidResponse(format!(
            "Anthropic model '{}' has no cached thinking capabilities; cannot apply reasoning level {reasoning}",
            protocol.model
        )));
    }
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
        // A fetched row with no thinking types leaves the model default.
        // Sending `enabled` 400s on current Claude 5 / 4.7+ models.
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
