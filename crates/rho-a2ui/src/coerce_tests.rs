use serde_json::json;

use super::*;

/// Interpolated numbers read the same in every renderer, which all
/// follow JavaScript's digits.
#[test]
fn numbers_are_written_as_javascript_writes_them() {
    let cases = [
        (98.5, "98.5"),
        (0.1 + 0.2, "0.30000000000000004"),
        (1e21, "1e+21"),
        (1e20, "100000000000000000000"),
        (1.5e-7, "1.5e-7"),
        (0.000_001, "0.000001"),
        (-2.5e-3, "-0.0025"),
        (123_456.789, "123456.789"),
    ];
    for (number, want) in cases {
        assert_eq!(number_string(number), want, "{number:e}");
    }
}

/// The blueprint's coercion table for strings and numbers. The
/// conformance suite's `and`, `or` and `not` cases cover booleans.
#[test]
fn coercion_table() {
    let to_number_cases = [
        (json!(null), 0.0),
        (json!(" 42.5 "), 42.5),
        (json!("abc"), 0.0),
        (json!(true), 1.0),
    ];
    for (value, want) in to_number_cases {
        assert!(
            (to_number(&value) - want).abs() < f64::EPSILON,
            "to_number({value})"
        );
    }
    let to_string_cases = [
        (json!(null), ""),
        (json!(5), "5"),
        (json!(98.5), "98.5"),
        (json!(false), "false"),
        (json!({"a": [1]}), r#"{"a":[1]}"#),
    ];
    for (value, want) in to_string_cases {
        assert_eq!(to_string(&value), want, "to_string({value})");
    }
}
