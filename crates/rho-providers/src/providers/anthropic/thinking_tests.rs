use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::provider_backend::ModelError;
use crate::providers::anthropic::DEFAULT_MAX_TOKENS;

fn adaptive_full_effort() -> Value {
    json!({
        "thinking": {
            "supported": true,
            "types": {
                "adaptive": {"supported": true},
                "enabled": {"supported": false}
            }
        },
        "effort": {
            "supported": true,
            "low": {"supported": true},
            "medium": {"supported": true},
            "high": {"supported": true},
            "xhigh": {"supported": true},
            "max": {"supported": true}
        }
    })
}

fn adaptive_without_xhigh() -> Value {
    json!({
        "thinking": {
            "supported": true,
            "types": {
                "adaptive": {"supported": true},
                "enabled": {"supported": false}
            }
        },
        "effort": {
            "supported": true,
            "low": {"supported": true},
            "medium": {"supported": true},
            "high": {"supported": true},
            "xhigh": {"supported": false},
            "max": {"supported": true}
        }
    })
}

fn enabled_budget() -> Value {
    json!({
        "thinking": {
            "supported": true,
            "types": {
                "adaptive": {"supported": false},
                "enabled": {"supported": true}
            }
        },
        "effort": {"supported": false}
    })
}

// Covers: unknown or missing capabilities must not send thinking.type.enabled
// Owner: anthropic thinking protocol
#[test]
fn unknown_capabilities_omit_thinking_instead_of_sending_a_budget() {
    let protocol = AnthropicThinkingProtocol::default();
    assert_eq!(
        thinking_config_for(
            "unknown-claude",
            &protocol,
            ReasoningLevel::Medium,
            DEFAULT_MAX_TOKENS,
        )
        .unwrap(),
        (None, None)
    );
}

// Covers: Opus 5 / Sonnet 5 class models advertise adaptive + effort
// Owner: anthropic thinking protocol
#[test]
fn adaptive_capabilities_send_effort_and_never_a_token_budget() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_full_effort());
    let (thinking, output) = thinking_config_for(
        "claude-opus-5",
        &protocol,
        ReasoningLevel::Medium,
        DEFAULT_MAX_TOKENS,
    )
    .unwrap();
    assert_eq!(
        thinking,
        Some(AnthropicThinkingConfig::Adaptive {
            display: "summarized"
        })
    );
    assert_eq!(output, Some(AnthropicOutputConfig { effort: "medium" }));
}

// Covers: Off on adaptive models that allow it uses thinking.type.disabled
// Owner: anthropic thinking protocol
#[test]
fn adaptive_off_disables_thinking_without_effort() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_full_effort());
    assert_eq!(
        thinking_config_for(
            "claude-opus-5",
            &protocol,
            ReasoningLevel::Off,
            DEFAULT_MAX_TOKENS,
        )
        .unwrap(),
        (Some(AnthropicThinkingConfig::Disabled), None)
    );
}

// Covers: Fable/Mythos reject thinking.type.disabled, so Off must not be sent
// Owner: anthropic thinking protocol
#[test]
fn fable_and_mythos_reject_reasoning_off() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_full_effort());
    for model in [
        "claude-fable-5",
        "claude-mythos-5",
        "claude-mythos-preview",
        "claude-fable-5-20260601",
    ] {
        assert!(
            matches!(
                thinking_config_for(model, &protocol, ReasoningLevel::Off, DEFAULT_MAX_TOKENS),
                Err(ModelError::UnsupportedReasoning { .. })
            ),
            "{model}"
        );
    }
}

// Covers: Haiku / Sonnet 4.5 still use budget tokens when that is advertised
// Owner: anthropic thinking protocol
#[test]
fn enabled_capabilities_reserve_an_answer_budget() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&enabled_budget());
    assert_eq!(
        thinking_config_for(
            "claude-haiku-4-5",
            &protocol,
            ReasoningLevel::Medium,
            DEFAULT_MAX_TOKENS,
        )
        .unwrap(),
        (
            Some(AnthropicThinkingConfig::Enabled {
                budget_tokens: DEFAULT_MAX_TOKENS - ANTHROPIC_ANSWER_RESERVE_TOKENS,
            }),
            None
        )
    );
}

// Covers: unsupported effort levels clamp to a supported neighbor
// Owner: anthropic thinking protocol
#[test]
fn effort_clamps_to_levels_the_model_advertises() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_without_xhigh());
    let (_, output) = thinking_config_for(
        "claude-opus-4-6",
        &protocol,
        ReasoningLevel::Xhigh,
        DEFAULT_MAX_TOKENS,
    )
    .unwrap();
    assert_eq!(output, Some(AnthropicOutputConfig { effort: "max" }));
}

#[test]
fn dated_snapshot_ids_reuse_the_parent_alias_capabilities() {
    assert_eq!(
        dated_parent_model("claude-opus-5-20260724"),
        Some("claude-opus-5")
    );
    assert_eq!(dated_parent_model("claude-opus-5"), None);
    assert_eq!(dated_parent_model("claude-sonnet-4-6"), None);
}
