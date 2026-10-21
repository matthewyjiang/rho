use pretty_assertions::assert_eq;
use serde_json::json;

use super::parse_usd_micros;

// Covers: numeric and formatted costs parse to micros, object-shaped
// usage.cost counts USD totals (not catalog rates), and malformed or
// out-of-range values are rejected
// Owner: shared USD cost parser
#[test]
fn parses_usd_micros_from_numbers_strings_and_usage_objects() {
    let cases = [
        ("number", json!(0.0042), Some(4_200)),
        ("formatted dollars", json!("$1,234.50"), Some(1_234_500_000)),
        ("decimal string", json!("1234.50"), Some(1_234_500_000)),
        ("negative number", json!(-1), None),
        ("doubled dollar sign", json!("$$1"), None),
        ("misplaced separator", json!("1,2"), None),
        ("out of range", json!("1e308"), None),
        ("prose", json!("not a cost"), None),
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
        (
            "negative input does not offset a valid output",
            json!({ "input_usd": -1.0, "output_usd": 0.0042 }),
            Some(4_200),
        ),
        (
            "negative preferred total falls back to a valid alias",
            json!({ "total_usd": -1.0, "cost_usd": 0.0042 }),
            Some(4_200),
        ),
        (
            "negative preferred total falls back to valid parts",
            json!({ "total_usd": -0.5, "input_usd": 0.001, "output_usd": 0.0032 }),
            Some(4_200),
        ),
    ];

    for (name, value, expected) in cases {
        assert_eq!(parse_usd_micros(&value), expected, "{name}");
    }
}
