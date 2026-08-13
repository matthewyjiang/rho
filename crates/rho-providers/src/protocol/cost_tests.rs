use pretty_assertions::assert_eq;
use serde_json::json;

use super::parse_usd_micros;

#[test]
fn parses_numeric_and_formatted_costs() {
    assert_eq!(parse_usd_micros(&json!(0.0042)), Some(4_200));
    assert_eq!(parse_usd_micros(&json!("$1,234.50")), Some(1_234_500_000));
    assert_eq!(parse_usd_micros(&json!("1234.50")), Some(1_234_500_000));
}

// Covers: object-shaped usage.cost must count USD totals, not catalog rates
// Owner: shared USD cost parser
#[test]
fn parses_object_shaped_usage_cost_totals() {
    let cases = [
        (
            "composer-api total_usd wins over parts",
            json!({
                "currency": "USD",
                "estimated": true,
                "input_usd": 0.001,
                "output_usd": 0.0032,
                "total_usd": 0.0042,
                "pricing": {
                    "input_per_million_tokens_usd": 0.5,
                    "output_per_million_tokens_usd": 2.5
                }
            }),
            Some(4_200),
        ),
        (
            "input_usd plus output_usd when total is absent",
            json!({ "input_usd": 0.001, "output_usd": 0.0032 }),
            Some(4_200),
        ),
        (
            "nested cost_usd alias",
            json!({ "cost_usd": "$0.0042" }),
            Some(4_200),
        ),
        (
            "zero total is preserved",
            json!({ "total_usd": 0 }),
            Some(0),
        ),
        (
            "catalog rates are not dollar totals",
            json!({ "input": 0.5, "output": 2.5 }),
            None,
        ),
    ];

    for (name, value, expected) in cases {
        assert_eq!(parse_usd_micros(&value), expected, "{name}");
    }
}

#[test]
fn rejects_invalid_or_out_of_range_costs() {
    for value in [
        json!(-1),
        json!("$$1"),
        json!("1,2"),
        json!("1e308"),
        json!("not a cost"),
    ] {
        assert_eq!(parse_usd_micros(&value), None, "value: {value}");
    }
}
