use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::provider_backend::ModelError;
use crate::providers::anthropic::DEFAULT_MAX_TOKENS;

fn adaptive_full_effort() -> serde_json::Value {
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

fn adaptive_without_xhigh() -> serde_json::Value {
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

fn enabled_budget() -> serde_json::Value {
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

fn capabilities_with_disabled_leaf(supported: bool) -> serde_json::Value {
    json!({
        "thinking": {
            "supported": true,
            "types": {
                "adaptive": {"supported": true},
                "enabled": {"supported": false},
                "disabled": {"supported": supported}
            }
        }
    })
}

fn config(
    model: &str,
    capabilities: &serde_json::Value,
    reasoning: ReasoningLevel,
) -> Result<
    (
        Option<AnthropicThinkingConfig>,
        Option<AnthropicOutputConfig>,
    ),
    ModelError,
> {
    thinking_config_for(
        &AnthropicThinkingProtocol::from_capabilities(model, capabilities),
        reasoning,
        DEFAULT_MAX_TOKENS,
    )
}

// Covers: missing capabilities must not silently drop an explicit reasoning
// request, and must not send thinking.type.enabled
// Owner: anthropic thinking protocol
#[test]
fn unresolved_capabilities_reject_requested_reasoning_and_omit_off() {
    let unknown = AnthropicThinkingProtocol::unknown("unknown-claude");
    assert!(matches!(
        thinking_config_for(&unknown, ReasoningLevel::Medium, DEFAULT_MAX_TOKENS),
        Err(ModelError::InvalidResponse(_))
    ));
    assert_eq!(
        thinking_config_for(&unknown, ReasoningLevel::Off, DEFAULT_MAX_TOKENS).unwrap(),
        (None, None)
    );
}

// Covers: a fetched empty capabilities object is resolved, not missing
// Owner: anthropic thinking protocol
#[test]
fn empty_fetched_capabilities_omit_thinking_without_error() {
    assert_eq!(
        config("claude-haiku-4-5", &json!({}), ReasoningLevel::Medium).unwrap(),
        (None, None)
    );
}

// Covers: Opus 5 / Sonnet 5 class models advertise adaptive + effort
// Owner: anthropic thinking protocol
#[test]
fn adaptive_capabilities_send_effort_and_never_a_token_budget() {
    let (thinking, output) = config(
        "claude-opus-5",
        &adaptive_full_effort(),
        ReasoningLevel::Medium,
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

// Covers: Off follows a disabled leaf when present, otherwise the tiny
// Models API gap table, and never sends xhigh/max with Off
// Owner: anthropic thinking protocol
#[test]
fn off_follows_disabled_leaf_then_model_gap_table() {
    let cases = [
        (
            "advertised disabled",
            "any-model",
            capabilities_with_disabled_leaf(/*supported*/ true),
            Ok(Some(AnthropicThinkingConfig::Disabled)),
        ),
        (
            "advertised cannot disable",
            "any-model",
            capabilities_with_disabled_leaf(/*supported*/ false),
            Err(()),
        ),
        (
            "opus 5 gap table",
            "claude-opus-5",
            adaptive_full_effort(),
            Ok(Some(AnthropicThinkingConfig::Disabled)),
        ),
        (
            "opus 5 dated snapshot gap table",
            "claude-opus-5-20260724",
            adaptive_full_effort(),
            Ok(Some(AnthropicThinkingConfig::Disabled)),
        ),
        (
            "sonnet 5 gap table",
            "claude-sonnet-5",
            adaptive_full_effort(),
            Ok(Some(AnthropicThinkingConfig::Disabled)),
        ),
        (
            "fable cannot disable",
            "claude-fable-5",
            adaptive_full_effort(),
            Err(()),
        ),
        (
            "mythos cannot disable",
            "claude-mythos-preview",
            adaptive_full_effort(),
            Err(()),
        ),
        (
            "leaf wins over fable table",
            "claude-fable-5",
            capabilities_with_disabled_leaf(/*supported*/ true),
            Ok(Some(AnthropicThinkingConfig::Disabled)),
        ),
        (
            "leaf wins over opus table",
            "claude-opus-5",
            capabilities_with_disabled_leaf(/*supported*/ false),
            Err(()),
        ),
        (
            "adaptive without leaf or table",
            "claude-opus-4-8",
            adaptive_full_effort(),
            Ok(None),
        ),
        (
            "enabled budget only",
            "claude-haiku-4-5",
            enabled_budget(),
            Ok(None),
        ),
    ];

    for (name, model, capabilities, expected) in cases {
        let result = config(model, &capabilities, ReasoningLevel::Off);
        match expected {
            Ok(thinking) => {
                let (actual, output) =
                    result.unwrap_or_else(|error| panic!("{name}: unexpected error: {error}"));
                assert_eq!(actual, thinking, "{name}");
                assert_eq!(output, None, "{name}: Off must not send effort");
                if thinking == Some(AnthropicThinkingConfig::Disabled) {
                    assert_eq!(
                        serde_json::to_value(&actual).unwrap(),
                        json!({"type": "disabled"}),
                        "{name}"
                    );
                }
            }
            Err(()) => {
                assert!(
                    matches!(result, Err(ModelError::UnsupportedReasoning { .. })),
                    "{name}: expected unsupported Off, got {result:?}"
                );
            }
        }
    }

    assert_eq!(
        thinking_config_for(
            &AnthropicThinkingProtocol::unknown("claude-unknown"),
            ReasoningLevel::Off,
            DEFAULT_MAX_TOKENS,
        )
        .unwrap(),
        (None, None)
    );
}

// Covers: Haiku / Sonnet 4.5 still use budget tokens when that is advertised
// Owner: anthropic thinking protocol
#[test]
fn enabled_capabilities_reserve_an_answer_budget() {
    assert_eq!(
        config(
            "claude-haiku-4-5",
            &enabled_budget(),
            ReasoningLevel::Medium,
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

// Covers: unsupported effort levels clamp down to the nearest cheaper level
// so the user's request never silently escalates cost
// Owner: anthropic thinking protocol
#[test]
fn effort_clamps_down_to_levels_the_model_advertises() {
    let (_, output) = config(
        "claude-opus-4-6",
        &adaptive_without_xhigh(),
        ReasoningLevel::Xhigh,
    )
    .unwrap();
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
    let (_, output) = config("claude-opus-5", &capabilities, ReasoningLevel::Low).unwrap();
    assert_eq!(output, Some(AnthropicOutputConfig { effort: "medium" }));
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

            let expected = config(
                "claude-opus-5",
                &adaptive_full_effort(),
                ReasoningLevel::Medium,
            )
            .unwrap();
            assert_eq!(
                thinking_config_for(
                    &resolve_thinking_protocol("claude-opus-5"),
                    ReasoningLevel::Medium,
                    DEFAULT_MAX_TOKENS,
                )
                .unwrap(),
                expected
            );
            assert_eq!(
                thinking_config_for(
                    &resolve_thinking_protocol("claude-opus-5-20260724"),
                    ReasoningLevel::Medium,
                    DEFAULT_MAX_TOKENS,
                )
                .unwrap(),
                expected
            );
            assert_eq!(
                thinking_config_for(
                    &resolve_thinking_protocol("claude-unknown"),
                    ReasoningLevel::Off,
                    DEFAULT_MAX_TOKENS,
                )
                .unwrap(),
                (None, None)
            );
            assert!(matches!(
                thinking_config_for(
                    &resolve_thinking_protocol("claude-unknown"),
                    ReasoningLevel::Medium,
                    DEFAULT_MAX_TOKENS,
                ),
                Err(ModelError::InvalidResponse(_))
            ));
        },
    );
}
