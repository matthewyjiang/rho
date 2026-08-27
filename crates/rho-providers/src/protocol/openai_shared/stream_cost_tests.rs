use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    extract_generation_output_tokens, extract_raw_usage, extract_usage,
    resolve_generation_output_tokens, GenerationTokenContext, HiddenReasoningRisk,
};
use crate::model::ModelUsage;
use rho_sdk::model::GenerationOutputTokens;

const HIDDEN_UNLIKELY: GenerationTokenContext = GenerationTokenContext {
    reasoning_streamed: false,
    hidden_reasoning_risk: HiddenReasoningRisk::Unlikely,
};
const HIDDEN_POSSIBLE: GenerationTokenContext = GenerationTokenContext {
    reasoning_streamed: false,
    hidden_reasoning_risk: HiddenReasoningRisk::Possible,
};
const REASONING_STREAMED: GenerationTokenContext = GenerationTokenContext {
    reasoning_streamed: true,
    hidden_reasoning_risk: HiddenReasoningRisk::Possible,
};

#[test]
fn reported_cost_includes_byok_upstream_inference_cost() {
    let value = json!({
        "usage": {
            "prompt_tokens": 91,
            "completion_tokens": 38,
            "cost": 0.0005,
            "cost_details": {"upstream_inference_cost": 0.0000973}
        }
    });

    assert_eq!(
        extract_usage(&value),
        Some(ModelUsage {
            input_tokens: None,
            output_tokens: Some(38),
            total_tokens: Some(129),
            cost_usd_micros: Some(597),
            ..ModelUsage::default()
        })
    );
}

#[test]
fn reported_cost_accepts_strings_and_preserves_zero() {
    let string_cost = json!({"usage": {"cost": "$0.0042"}});
    let zero_cost = json!({"usage": {"cost": 0}});

    assert_eq!(
        extract_usage(&string_cost).and_then(|usage| usage.cost_usd_micros),
        Some(4_200)
    );
    assert_eq!(
        extract_usage(&zero_cost).and_then(|usage| usage.cost_usd_micros),
        Some(0)
    );
}

// Covers: custom OpenAI-compatible hosts (composer-api) report object usage.cost
// Owner: OpenAI shared usage parser
#[test]
fn reported_cost_reads_composer_api_usage_object() {
    let value = json!({
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 5,
            "total_tokens": 25,
            "prompt_tokens_details": { "cached_tokens": 0 },
            "cost": {
                "currency": "USD",
                "estimated": true,
                "input_usd": 0.00001,
                "output_usd": 0.0000125,
                "total_usd": 0.0000225,
                "pricing": {
                    "input_per_million_tokens_usd": 0.5,
                    "output_per_million_tokens_usd": 2.5,
                    "source": "https://cursor.com/changelog/composer-2-5"
                }
            }
        }
    });

    assert_eq!(
        extract_usage(&value),
        Some(ModelUsage {
            input_tokens: Some(20),
            output_tokens: Some(5),
            total_tokens: Some(25),
            cache_read_tokens: Some(0),
            cost_usd_micros: Some(23),
            ..ModelUsage::default()
        })
    );
}

#[test]
fn valid_cost_components_survive_missing_or_malformed_aliases() {
    let upstream_only = json!({
        "usage": {"cost_details": {"upstream_inference_cost": 0.0000973}}
    });
    let malformed_preferred_alias = json!({
        "usage": {"cost_usd": "invalid", "cost": 0.0042}
    });

    assert_eq!(
        extract_usage(&upstream_only).and_then(|usage| usage.cost_usd_micros),
        Some(97)
    );
    assert_eq!(
        extract_usage(&malformed_preferred_alias).and_then(|usage| usage.cost_usd_micros),
        Some(4_200)
    );
}

#[test]
fn invalid_reported_costs_do_not_replace_catalog_fallback() {
    let negative_cost = json!({"usage": {"cost": -1}});
    let malformed_cost = json!({"usage": {"cost": "not a cost"}});

    assert_eq!(
        extract_usage(&negative_cost).and_then(|usage| usage.cost_usd_micros),
        None
    );
    assert_eq!(
        extract_usage(&malformed_cost).and_then(|usage| usage.cost_usd_micros),
        None
    );
}

// Covers: a bare prompt_tokens total still includes cache hits, so it must not
// become ModelUsage.input_tokens (uncached) until the host reports the split
// Owner: OpenAI shared usage parser
#[test]
fn prompt_tokens_without_cache_details_are_not_uncached_input() {
    let unknown_split = json!({
        "usage": { "prompt_tokens": 100, "completion_tokens": 5, "total_tokens": 105 }
    });
    let explicit_zero_cache = json!({
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 0 }
        }
    });

    assert_eq!(
        extract_usage(&unknown_split),
        Some(ModelUsage {
            input_tokens: None,
            output_tokens: Some(5),
            total_tokens: Some(105),
            ..ModelUsage::default()
        })
    );
    assert_eq!(
        extract_usage(&explicit_zero_cache),
        Some(ModelUsage {
            input_tokens: Some(100),
            output_tokens: Some(5),
            cache_read_tokens: Some(0),
            total_tokens: Some(105),
            ..ModelUsage::default()
        })
    );
}

// Covers: the throughput numerator must match the generation window — subtract
// reasoning only when it stayed off the wire, refuse aggregate totals that may
// hide reasoning, and keep full totals when reasoning streamed in-window
// Owner: OpenAI shared usage parser
#[test]
fn generation_output_tokens_match_generation_window_accounting() {
    let cases = [
        (
            "responses aliases subtract hidden reasoning",
            json!({"usage": {
                "output_tokens": 30,
                "output_tokens_details": {"reasoning_tokens": 12}
            }}),
            HIDDEN_POSSIBLE,
            Some(GenerationOutputTokens::Reported(18)),
            Some(30),
        ),
        (
            "chat completions aliases subtract hidden reasoning",
            json!({"usage": {
                "completion_tokens": 21,
                "completion_tokens_details": {"reasoning_tokens": 8}
            }}),
            HIDDEN_POSSIBLE,
            Some(GenerationOutputTokens::Reported(13)),
            Some(21),
        ),
        (
            "streamed reasoning keeps the full total despite details",
            json!({"usage": {
                "completion_tokens": 160,
                "completion_tokens_details": {"reasoning_tokens": 100}
            }}),
            REASONING_STREAMED,
            Some(GenerationOutputTokens::Reported(160)),
            Some(160),
        ),
        (
            "streamed reasoning keeps the full total without details",
            json!({"usage": {"completion_tokens": 230}}),
            REASONING_STREAMED,
            Some(GenerationOutputTokens::Reported(230)),
            Some(230),
        ),
        (
            "details absent with possible hidden reasoning is unavailable",
            json!({"usage": {
                "prompt_tokens": 3211,
                "completion_tokens": 230,
                "total_tokens": 3441
            }}),
            HIDDEN_POSSIBLE,
            Some(GenerationOutputTokens::Unavailable),
            Some(230),
        ),
        (
            "details absent without reasoning trusts the aggregate",
            json!({"usage": {"output_tokens": 11}}),
            HIDDEN_UNLIKELY,
            None,
            Some(11),
        ),
        (
            "count and details aliases stay paired",
            json!({"usage": {
                "output_tokens": 30,
                "completion_tokens": 21,
                "completion_tokens_details": {"reasoning_tokens": 8}
            }}),
            HIDDEN_UNLIKELY,
            None,
            Some(30),
        ),
        (
            "malformed preferred aliases fall through",
            json!({"usage": {
                "output_tokens": "invalid",
                "completion_tokens": 19,
                "output_tokens_details": {"reasoning_tokens": "invalid"},
                "completion_tokens_details": {"reasoning_tokens": 4}
            }}),
            HIDDEN_POSSIBLE,
            Some(GenerationOutputTokens::Reported(15)),
            Some(19),
        ),
        (
            "reasoning cannot underflow output",
            json!({"usage": {
                "output_tokens": 3,
                "output_tokens_details": {"reasoning_tokens": 9}
            }}),
            HIDDEN_UNLIKELY,
            Some(GenerationOutputTokens::Unavailable),
            Some(3),
        ),
        (
            "output absent",
            json!({"usage": {
                "output_tokens_details": {"reasoning_tokens": 2}
            }}),
            HIDDEN_POSSIBLE,
            None,
            None,
        ),
        (
            "null usage placeholder chunks report nothing",
            json!({"usage": null}),
            HIDDEN_POSSIBLE,
            None,
            None,
        ),
    ];

    for (name, value, context, expected_generation, expected_usage) in cases {
        assert_eq!(
            extract_generation_output_tokens(&value, context),
            expected_generation,
            "{name}: generation output"
        );
        assert_eq!(
            extract_usage(&value).and_then(|usage| usage.output_tokens),
            expected_usage,
            "{name}: aggregate usage"
        );
    }
}

// Covers: a later usage snapshot that reports output without reasoning
// details must replace the whole output/reasoning pair, not keep the
// earlier reasoning count next to the new output total
// Owner: OpenAI shared usage parser
#[test]
fn merge_replaces_output_reasoning_pair_atomically() {
    let earlier = extract_raw_usage(&json!({"usage": {
        "completion_tokens": 30,
        "completion_tokens_details": {"reasoning_tokens": 12}
    }}))
    .unwrap();
    let later = extract_raw_usage(&json!({"usage": {"completion_tokens": 40}})).unwrap();

    assert_eq!(
        resolve_generation_output_tokens(earlier.merge(later).reported_output(), HIDDEN_POSSIBLE),
        Some(GenerationOutputTokens::Unavailable)
    );
}
