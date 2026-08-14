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

// Covers: Off on adaptive capabilities, including Opus 5 dated snapshots,
// must serialize thinking.type=disabled and must not send xhigh/max effort
// Owner: anthropic thinking protocol
#[test]
fn off_serializes_disabled_from_adaptive_capabilities() {
    let cache = tempfile::tempdir().unwrap();
    crate::model::provider_models::with_provider_models_cache_dir_for_tests(
        cache.path().to_path_buf(),
        || {
            crate::model::provider_models::write_cached_provider_model_raw_json_for_tests(
                "anthropic",
                "claude-opus-5",
                "Claude Opus 5",
                &adaptive_full_effort(),
            )
            .unwrap();

            let cases = [
                (
                    "opus-5 alias",
                    resolve_thinking_protocol("claude-opus-5"),
                    true,
                ),
                (
                    "opus-5 dated snapshot",
                    resolve_thinking_protocol("claude-opus-5-20260724"),
                    true,
                ),
                (
                    "enabled budget only",
                    AnthropicThinkingProtocol::from_capabilities(&enabled_budget()),
                    false,
                ),
                (
                    "unknown capabilities",
                    AnthropicThinkingProtocol::default(),
                    false,
                ),
            ];
            for (name, protocol, expect_disabled) in cases {
                let (thinking, output) =
                    thinking_config_for(&protocol, ReasoningLevel::Off, DEFAULT_MAX_TOKENS)
                        .unwrap();
                if expect_disabled {
                    assert_eq!(thinking, Some(AnthropicThinkingConfig::Disabled), "{name}");
                    assert_eq!(output, None, "{name}: Off must not send xhigh/max effort");
                    assert_eq!(
                        serde_json::to_value(&thinking).unwrap(),
                        json!({"type": "disabled"}),
                        "{name}"
                    );
                } else {
                    assert_eq!((thinking, output), (None, None), "{name}");
                }
            }
        },
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

// Covers: unsupported effort levels clamp down to the nearest cheaper level
// so the user's request never silently escalates cost
// Owner: anthropic thinking protocol
#[test]
fn effort_clamps_down_to_levels_the_model_advertises() {
    let protocol = AnthropicThinkingProtocol::from_capabilities(&adaptive_without_xhigh());
    let (_, output) =
        thinking_config_for(&protocol, ReasoningLevel::Xhigh, DEFAULT_MAX_TOKENS).unwrap();
    assert_eq!(output, Some(AnthropicOutputConfig { effort: "high" }));
}

// Covers: a request below the advertised range rises to the model minimum
// Owner: anthropic thinking protocol
#[test]
fn effort_below_the_advertised_range_rises_to_the_model_minimum() {
    let capabilities = json!({
        "thinking": {
            "supported": true,
            "types": {"adaptive": {"supported": true}}
        },
        "effort": {
            "supported": true,
            "low": {"supported": false},
            "medium": {"supported": true},
            "high": {"supported": true}
        }
    });
    let protocol = AnthropicThinkingProtocol::from_capabilities(&capabilities);
    let (_, output) =
        thinking_config_for(&protocol, ReasoningLevel::Low, DEFAULT_MAX_TOKENS).unwrap();
    assert_eq!(output, Some(AnthropicOutputConfig { effort: "medium" }));
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

// Covers: construction-time resolution reads the cached capabilities row and
// falls back to the parent alias for dated snapshot ids
// Owner: anthropic thinking protocol
#[test]
fn resolve_reads_cached_capabilities_including_dated_snapshots() {
    let cache = tempfile::tempdir().unwrap();
    crate::model::provider_models::with_provider_models_cache_dir_for_tests(
        cache.path().to_path_buf(),
        || {
            crate::model::provider_models::write_cached_provider_model_raw_json_for_tests(
                "anthropic",
                "claude-opus-5",
                "Claude Opus 5",
                &adaptive_full_effort(),
            )
            .unwrap();

            let expected = AnthropicThinkingProtocol::from_capabilities(&adaptive_full_effort());
            assert_eq!(resolve_thinking_protocol("claude-opus-5"), expected);
            assert_eq!(
                resolve_thinking_protocol("claude-opus-5-20260724"),
                expected
            );
            assert_eq!(
                resolve_thinking_protocol("claude-unknown"),
                AnthropicThinkingProtocol::default()
            );
        },
    );
}
