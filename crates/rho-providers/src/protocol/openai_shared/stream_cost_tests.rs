use pretty_assertions::assert_eq;
use serde_json::json;

use super::{classify_generation_output_tokens, extract_usage, GenerationOutputTokens};
use crate::model::ModelUsage;
use crate::protocol::openai_chat::HiddenReasoningRisk;

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
            input_tokens: Some(91),
            output_tokens: Some(38),
            total_tokens: None,
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

// Covers: throughput must exclude reasoning tokens across ordered OpenAI usage aliases
// Owner: OpenAI shared usage parser
#[test]
fn generation_output_tokens_exclude_reasoning_across_usage_aliases() {
    let cases = [
        (
            "responses aliases",
            json!({"usage": {
                "output_tokens": 30,
                "output_tokens_details": {"reasoning_tokens": 12}
            }}),
            GenerationOutputTokens::Reported(18),
            Some(30),
        ),
        (
            "chat completions aliases",
            json!({"usage": {
                "completion_tokens": 21,
                "completion_tokens_details": {"reasoning_tokens": 8}
            }}),
            GenerationOutputTokens::Reported(13),
            Some(21),
        ),
        (
            "count and details aliases stay paired",
            json!({"usage": {
                "output_tokens": 30,
                "completion_tokens": 21,
                "completion_tokens_details": {"reasoning_tokens": 8}
            }}),
            GenerationOutputTokens::Unreported,
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
            GenerationOutputTokens::Reported(15),
            Some(19),
        ),
        (
            "reasoning cannot underflow output",
            json!({"usage": {
                "output_tokens": 3,
                "output_tokens_details": {"reasoning_tokens": 9}
            }}),
            GenerationOutputTokens::Invalid,
            Some(3),
        ),
        (
            "output absent",
            json!({"usage": {
                "output_tokens_details": {"reasoning_tokens": 2}
            }}),
            GenerationOutputTokens::Unreported,
            None,
        ),
        (
            "details absent",
            json!({"usage": {"output_tokens": 11}}),
            GenerationOutputTokens::Unreported,
            Some(11),
        ),
    ];

    for (name, value, expected_generation, expected_usage) in cases {
        assert_eq!(
            classify_generation_output_tokens(
                &value,
                HiddenReasoningRisk::Unlikely,
                /*reasoning_streamed*/ false
            ),
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

// Covers: known thinking without a reasoning-token count cannot use aggregate
// output as a generation-throughput numerator.
// Owner: OpenAI shared usage parser
#[test]
fn omitted_reasoning_count_is_unavailable_when_thinking_is_known() {
    let usage = json!({"usage": {"output_tokens": 11}});
    assert_eq!(
        classify_generation_output_tokens(&usage, HiddenReasoningRisk::Likely, false),
        GenerationOutputTokens::Invalid
    );
    assert_eq!(
        classify_generation_output_tokens(&usage, HiddenReasoningRisk::Likely, true),
        GenerationOutputTokens::Invalid
    );
    assert_eq!(
        classify_generation_output_tokens(&usage, HiddenReasoningRisk::Unlikely, true),
        GenerationOutputTokens::Invalid
    );
    assert_eq!(
        classify_generation_output_tokens(&usage, HiddenReasoningRisk::Unlikely, false),
        GenerationOutputTokens::Unreported
    );
}
