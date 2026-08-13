use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
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
        thinking_config_for(&protocol, ReasoningLevel::Medium, DEFAULT_MAX_TOKENS).unwrap(),
        (None, None)
    );
}

// Covers: Opus 5 / Sonnet 5 class models advertise adaptive + effort
// Owner: anthropic thinking protocol
#[test]
fn adaptive_capabilities_send_effort_and_never_a_token_budget() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_full_effort());
    let (thinking, output) =
        thinking_config_for(&protocol, ReasoningLevel::Medium, DEFAULT_MAX_TOKENS).unwrap();
    assert_eq!(
        thinking,
        Some(AnthropicThinkingConfig::Adaptive {
            display: "summarized"
        })
    );
    assert_eq!(output, Some(AnthropicOutputConfig { effort: "medium" }));
}

// Covers: Off on adaptive models uses thinking.type.disabled
// Owner: anthropic thinking protocol
#[test]
fn adaptive_off_disables_thinking_without_effort() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_full_effort());
    assert_eq!(
        thinking_config_for(&protocol, ReasoningLevel::Off, DEFAULT_MAX_TOKENS).unwrap(),
        (Some(AnthropicThinkingConfig::Disabled), None)
    );
}

// Covers: Haiku / Sonnet 4.5 still use budget tokens when that is advertised
// Owner: anthropic thinking protocol
#[test]
fn enabled_capabilities_reserve_an_answer_budget() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&enabled_budget());
    assert_eq!(
        thinking_config_for(&protocol, ReasoningLevel::Medium, DEFAULT_MAX_TOKENS).unwrap(),
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
    let (_, output) =
        thinking_config_for(&protocol, ReasoningLevel::Xhigh, DEFAULT_MAX_TOKENS).unwrap();
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
