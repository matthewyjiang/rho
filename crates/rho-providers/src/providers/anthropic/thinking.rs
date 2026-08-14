use crate::{
    model::provider_models::{self, AnthropicThinkingMode, OffThinking},
    protocol::anthropic_messages::{AnthropicOutputConfig, AnthropicThinkingConfig},
    provider_backend::ModelError,
    reasoning::ReasoningLevel,
};

use super::ANTHROPIC_ANSWER_RESERVE_TOKENS;

/// Construction-time thinking source for one Anthropic model.
///
/// `None` mode means no cached Models API row. That is distinct from
/// [`AnthropicThinkingMode::Unknown`], which is a fetched object that does not
/// identify adaptive or budget control — incomplete, not non-configurable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThinkingSource {
    model: String,
    mode: Option<AnthropicThinkingMode>,
}

impl ThinkingSource {
    pub(super) fn resolve(provider: &str, model: &str) -> Self {
        Self {
            model: model.to_string(),
            mode: thinking_mode_for_provider(provider, model),
        }
    }

    #[cfg(test)]
    pub(crate) fn unresolved(model: &str) -> Self {
        Self {
            model: model.to_string(),
            mode: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_capabilities(model: &str, capabilities: &serde_json::Value) -> Self {
        Self {
            model: model.to_string(),
            mode: provider_models::anthropic_thinking_mode_from_value(model, capabilities),
        }
    }

    fn unsupported(&self, requested: ReasoningLevel) -> ModelError {
        ModelError::UnsupportedReasoning {
            provider: "anthropic",
            model: self.model.clone(),
            requested,
        }
    }
}

fn thinking_mode_for_provider(provider: &str, model: &str) -> Option<AnthropicThinkingMode> {
    if provider == "anthropic" {
        return provider_models::cached_anthropic_thinking_mode(model);
    }
    Some(
        crate::model::models_dev::cached_model_metadata(provider, model)
            .map(|metadata| {
                AnthropicThinkingMode::from_host_catalog_capabilities(
                    metadata.reasoning_capabilities(),
                )
            })
            .unwrap_or(AnthropicThinkingMode::BudgetTokens {
                off: OffThinking::Disabled,
            }),
    )
}

pub(super) fn thinking_config_for(
    source: &ThinkingSource,
    reasoning: ReasoningLevel,
    max_tokens: u32,
) -> Result<
    (
        Option<AnthropicThinkingConfig>,
        Option<AnthropicOutputConfig>,
    ),
    ModelError,
> {
    let Some(mode) = source.mode else {
        // Cold cache: Off omits thinking so we never send type=enabled by
        // default. Any explicit non-Off level needs a known mode.
        return if reasoning == ReasoningLevel::Off {
            Ok((None, None))
        } else {
            Err(source.unsupported(reasoning))
        };
    };

    if reasoning == ReasoningLevel::Off {
        // Off never sends output_config, so xhigh/max cannot ride along.
        return match mode.off() {
            OffThinking::Unsupported => Err(source.unsupported(reasoning)),
            OffThinking::Disabled => Ok((Some(AnthropicThinkingConfig::Disabled), None)),
            OffThinking::Omit => Ok((None, None)),
        };
    }

    match mode {
        AnthropicThinkingMode::Adaptive { efforts, .. } => Ok((
            Some(AnthropicThinkingConfig::Adaptive {
                display: "summarized",
            }),
            efforts.map(|mask| AnthropicOutputConfig {
                effort: mask.for_level(reasoning),
            }),
        )),
        AnthropicThinkingMode::BudgetTokens { .. } => {
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
        // Fetched but incomplete: leave the model default. Do not invent
        // type=enabled (400s on current Claude 5) or fail closed as if the API
        // proved reasoning is unsupported.
        AnthropicThinkingMode::Unknown { .. } => Ok((None, None)),
    }
}

#[cfg(test)]
#[path = "thinking_tests.rs"]
mod tests;
