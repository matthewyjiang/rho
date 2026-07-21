use pretty_assertions::assert_eq;
use serde_json::json;

use super::parse_usd_micros;

#[test]
fn parses_numeric_and_formatted_costs() {
    assert_eq!(parse_usd_micros(&json!(0.0042)), Some(4_200));
    assert_eq!(parse_usd_micros(&json!("$1,234.50")), Some(1_234_500_000));
    assert_eq!(parse_usd_micros(&json!("1234.50")), Some(1_234_500_000));
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
