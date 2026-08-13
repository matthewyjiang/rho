use serde_json::Value;

use crate::{
    model::provider_models,
    protocol::anthropic_messages::{AnthropicOutputConfig, AnthropicThinkingConfig},
    provider_backend::ModelError,
    reasoning::ReasoningLevel,
};

use super::ANTHROPIC_ANSWER_RESERVE_TOKENS;

/// Wire protocol advertised by Anthropic's Models API `capabilities` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AnthropicThinkingProtocol {
    adaptive: bool,
    enabled: bool,
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
    pub(crate) fn from_capabilities(capabilities: &Value) -> Self {
        let thinking = capabilities.get("thinking");
        let effort = capabilities.get("effort");
        Self {
            adaptive: leaf_supported(thinking, &["types", "adaptive"]),
            enabled: leaf_supported(thinking, &["types", "enabled"]),
            effort: EffortSupport {
                supported: leaf_supported(effort, &[]),
                levels: EFFORT_LEVELS.map(|level| leaf_supported(effort, &[level])),
            },
        }
    }
}

/// Resolves the cached Models API capabilities for `model`, falling back to the
/// parent alias for dated snapshot ids. Resolved once at provider construction;
/// request building stays pure.
pub(super) fn resolve_thinking_protocol(model: &str) -> AnthropicThinkingProtocol {
    let capabilities = cached_capabilities(model)
        .or_else(|| dated_parent_model(model).and_then(cached_capabilities));
    match capabilities {
        Some(value) => AnthropicThinkingProtocol::from_capabilities(&value),
        None => {
            tracing::warn!(
                target: "rho::providers",
                "no cached Anthropic capabilities for model {model}; reasoning levels will not change thinking on the wire"
            );
            AnthropicThinkingProtocol::default()
        }
    }
}

fn cached_capabilities(model: &str) -> Option<Value> {
    provider_models::cached_provider_model_raw_json("anthropic", model)
        .filter(|value| !value.is_null())
}

fn dated_parent_model(model: &str) -> Option<&str> {
    let (parent, date) = model.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit())).then_some(parent)
}

// The Models API capabilities object has no `disabled` leaf, so which models
// accept or require `thinking.type.disabled` stays hardcoded by model family.
fn adaptive_thinking_is_mandatory(model: &str) -> bool {
    model_in_families(
        model,
        &["claude-fable-5", "claude-mythos-5", "claude-mythos-preview"],
    )
}

fn supports_disabled_thinking(model: &str) -> bool {
    model_in_families(model, &["claude-sonnet-5"])
}

fn model_in_families(model: &str, families: &[&str]) -> bool {
    let canonical = dated_parent_model(model).unwrap_or(model);
    families.iter().any(|prefix| {
        canonical == *prefix
            || canonical
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('-'))
    })
}

pub(super) fn thinking_config_for(
    model: &str,
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
        if adaptive_thinking_is_mandatory(model) {
            return Err(ModelError::UnsupportedReasoning {
                provider: "anthropic",
                model: model.to_string(),
                requested: reasoning,
            });
        }
        // Only models known to accept thinking.type.disabled get it; sending
        // the field to a model that rejects it is a 400, while omitting it
        // merely leaves thinking at the model default.
        let thinking =
            supports_disabled_thinking(model).then_some(AnthropicThinkingConfig::Disabled);
        return Ok((thinking, None));
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

fn leaf_supported(root: Option<&Value>, path: &[&str]) -> bool {
    let Some(mut current) = root else {
        return false;
    };
    for key in path {
        let Some(next) = current.get(*key) else {
            return false;
        };
        current = next;
    }
    current
        .get("supported")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "thinking_tests.rs"]
mod tests;
