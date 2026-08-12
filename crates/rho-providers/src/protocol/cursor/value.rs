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
        Some(Kind::NumberValue(number)) => Number::from_f64(*number)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
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
