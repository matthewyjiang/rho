use prost_types::value::Kind;
use prost_types::{ListValue, NullValue, Struct, Value as ProtoValue};
use serde_json::{Map, Number, Value as JsonValue};

pub(crate) fn protobuf_value_from_json(value: &JsonValue) -> ProtoValue {
    ProtoValue {
        kind: Some(match value {
            JsonValue::Null => Kind::NullValue(NullValue::NullValue as i32),
            JsonValue::Bool(flag) => Kind::BoolValue(*flag),
            JsonValue::Number(number) => Kind::NumberValue(number.as_f64().unwrap_or(0.0)),
            JsonValue::String(text) => Kind::StringValue(text.clone()),
            JsonValue::Array(values) => Kind::ListValue(ListValue {
                values: values.iter().map(protobuf_value_from_json).collect(),
            }),
            JsonValue::Object(fields) => Kind::StructValue(Struct {
                fields: fields
                    .iter()
                    .map(|(key, value)| (key.clone(), protobuf_value_from_json(value)))
                    .collect(),
            }),
        }),
    }
}

pub(crate) fn json_from_protobuf_value(value: &ProtoValue) -> JsonValue {
    match value.kind.as_ref() {
        None | Some(Kind::NullValue(_)) => JsonValue::Null,
        Some(Kind::BoolValue(flag)) => JsonValue::Bool(*flag),
        Some(Kind::NumberValue(number)) => json_number_from_f64(*number),
        Some(Kind::StringValue(text)) => JsonValue::String(text.clone()),
        Some(Kind::ListValue(list)) => {
            JsonValue::Array(list.values.iter().map(json_from_protobuf_value).collect())
        }
        Some(Kind::StructValue(object)) => JsonValue::Object(
            object
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), json_from_protobuf_value(value)))
                .collect::<Map<_, _>>(),
        ),
    }
}

/// Cursor encodes every number as protobuf `double`. Whole values become JSON
/// integers so `u64` tool fields such as `timeout_seconds` parse.
pub(crate) fn canonicalize_json_numbers(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Number(number) if number.is_i64() || number.is_u64() => {
            JsonValue::Number(number)
        }
        JsonValue::Number(number) => number
            .as_f64()
            .map(json_number_from_f64)
            .unwrap_or(JsonValue::Number(number)),
        JsonValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(canonicalize_json_numbers).collect())
        }
        JsonValue::Object(fields) => JsonValue::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}

fn json_number_from_f64(value: f64) -> JsonValue {
    if let Some(as_u64) = whole_u64_from_f64(value) {
        return JsonValue::Number(as_u64.into());
    }
    if value.is_finite() && value < 0.0 {
        let as_i64 = value as i64;
        if as_i64 as f64 == value {
            return JsonValue::Number(as_i64.into());
        }
    }
    Number::from_f64(value)
        .map(JsonValue::Number)
        .unwrap_or(JsonValue::Null)
}

fn whole_u64_from_f64(value: f64) -> Option<u64> {
    if !value.is_finite() || !(0.0..=u64::MAX as f64).contains(&value) {
        return None;
    }
    let as_u64 = value as u64;
    (as_u64 as f64 == value).then_some(as_u64)
}

#[cfg(test)]
#[path = "value_tests.rs"]
mod tests;
