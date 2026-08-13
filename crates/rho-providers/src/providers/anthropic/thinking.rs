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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EffortSupport {
    supported: bool,
    low: bool,
    medium: bool,
    high: bool,
    xhigh: bool,
    max: bool,
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
                low: leaf_supported(effort, &["low"]),
                medium: leaf_supported(effort, &["medium"]),
                high: leaf_supported(effort, &["high"]),
                xhigh: leaf_supported(effort, &["xhigh"]),
                max: leaf_supported(effort, &["max"]),
            },
        }
    }
}

pub(crate) fn thinking_config(
    model: &str,
    reasoning: ReasoningLevel,
    max_tokens: u32,
) -> Result<
    (
        Option<AnthropicThinkingConfig>,
        Option<AnthropicOutputConfig>,
    ),
    ModelError,
> {
    thinking_config_for(&resolve_thinking_protocol(model), reasoning, max_tokens)
}

fn resolve_thinking_protocol(model: &str) -> AnthropicThinkingProtocol {
    cached_capabilities(model)
        .or_else(|| dated_parent_model(model).and_then(cached_capabilities))
        .as_ref()
        .map(AnthropicThinkingProtocol::from_capabilities)
        .unwrap_or_default()
}

fn cached_capabilities(model: &str) -> Option<Value> {
    provider_models::cached_provider_model_raw_json("anthropic", model)
        .filter(|value| !value.is_null())
}

fn dated_parent_model(model: &str) -> Option<&str> {
    let (parent, date) = model.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit())).then_some(parent)
}

fn thinking_config_for(
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
        // Adaptive models accept disabled without an effort field. Mandatory
        // models (Fable/Mythos) reject it; Anthropic does not advertise that
        // yet, so Off still sends disabled when adaptive is known.
        let thinking = protocol
            .adaptive
            .then_some(AnthropicThinkingConfig::Disabled);
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
        ReasoningLevel::Off => return Ok((None, None)),
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
    fn for_level(self, reasoning: ReasoningLevel) -> Option<&'static str> {
        if !self.supported {
            return None;
        }
        let requested = match reasoning {
            ReasoningLevel::Off | ReasoningLevel::Minimal | ReasoningLevel::Low => "low",
            ReasoningLevel::Medium => "medium",
            ReasoningLevel::High => "high",
            ReasoningLevel::Xhigh => "xhigh",
            ReasoningLevel::Max => "max",
        };
        self.supported_level(requested)
    }

    fn supported_level(self, requested: &'static str) -> Option<&'static str> {
        const ORDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];
        if self.allows(requested) {
            return Some(requested);
        }
        let index = ORDER.iter().position(|level| *level == requested)?;
        ORDER[index + 1..]
            .iter()
            .copied()
            .find(|level| self.allows(level))
            .or_else(|| {
                ORDER[..index]
                    .iter()
                    .rev()
                    .copied()
                    .find(|level| self.allows(level))
            })
    }

    fn allows(self, level: &str) -> bool {
        match level {
            "low" => self.low,
            "medium" => self.medium,
            "high" => self.high,
            "xhigh" => self.xhigh,
            "max" => self.max,
            _ => false,
        }
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
