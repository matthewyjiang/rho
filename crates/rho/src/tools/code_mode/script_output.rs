//! Script result envelope: host status and model text never overwrite tool data.

use rho_sdk::tool::ToolOutput;
use serde_json::{json, Value};

/// Every call returns `{is_error, content, data}`. `data` is nullable because
/// text-only tools and oversized structured results have no retained JSON.
/// Successful data follows the tool's schema; failed server payloads need not.
pub(crate) fn schema(data_schema: Option<Value>) -> Value {
    let mut envelope = json!({
        "type": "object",
        "properties": {
            "is_error": {"type": "boolean"},
            "content": {"type": "string"},
            "data": {}
        },
        "required": ["is_error", "content", "data"],
        "additionalProperties": false
    });
    if let Some(mut data_schema) = data_schema {
        // Keep server-local references rooted in the embedded schema resource,
        // rather than accidentally resolving against the envelope's root.
        if let Some(object) = data_schema.as_object_mut() {
            object
                .entry("$id")
                .or_insert(json!("urn:rho:script-tool-data"));
        }
        envelope["if"] = json!({"properties": {"is_error": {"const": false}}});
        envelope["then"] =
            json!({"properties": {"data": {"anyOf": [data_schema, {"type": "null"}]}}});
    }
    envelope
}

pub(crate) fn value(output: &ToolOutput) -> Value {
    json!({
        "is_error": output.is_failure(),
        "content": output.content(),
        "data": output.structured_content(),
    })
}

/// Envelope for a batched call that failed before producing output.
pub(crate) fn error_value(message: &str) -> Value {
    json!({"is_error": true, "content": message, "data": null})
}
