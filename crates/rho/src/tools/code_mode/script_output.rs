//! Script result envelope: host status and model text never overwrite tool data.

use rho_sdk::tool::ToolOutput;
use serde_json::{json, Value};

/// Every call returns `{is_error, content, data}`. `data` is null when the
/// tool returned text only or its structured result was too large to retain.
/// Successful data follows the tool's `returns` schema; failed server
/// payloads need not.
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
