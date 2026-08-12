use pretty_assertions::assert_eq;
use prost_types::value::Kind;
use prost_types::Value as ProtoValue;
use serde_json::{json, Number, Value as JsonValue};

use super::{canonicalize_json_numbers, json_from_protobuf_value};

fn proto_number(value: f64) -> ProtoValue {
    ProtoValue {
        kind: Some(Kind::NumberValue(value)),
    }
}

fn json_float(value: f64) -> JsonValue {
    JsonValue::Number(Number::from_f64(value).expect("finite"))
}

// Covers: Cursor MCP numbers are protobuf doubles; whole values must become JSON
// integers so u64 tool fields parse.
// Owner: cursor protocol
#[test]
fn whole_protobuf_numbers_become_json_integers() {
    let cases = [
        (30.0, json!(30_u64)),
        (0.0, json!(0_u64)),
        (-2.0, json!(-2_i64)),
        (1.5, json_float(1.5)),
    ];

    for (input, expected) in cases {
        assert_eq!(
            json_from_protobuf_value(&proto_number(input)),
            expected,
            "protobuf number {input}"
        );
    }
}

// Covers: raw JSON floats on the MCP fallback path must canonicalize the same way
// Owner: cursor protocol
#[test]
fn canonicalize_json_numbers_promotes_whole_floats() {
    let value = json!({
        "timeout_seconds": json_float(30.0),
        "nested": [json_float(1.0), json_float(1.5)],
    });

    assert_eq!(
        canonicalize_json_numbers(value),
        json!({
            "timeout_seconds": 30_u64,
            "nested": [1_u64, json_float(1.5)],
        })
    );
}
