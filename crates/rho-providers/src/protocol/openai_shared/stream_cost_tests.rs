use pretty_assertions::assert_eq;
use serde_json::json;

use super::extract_usage;
use crate::model::ModelUsage;

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
