use prost::Message;
use serde_json::{Map, Value};

use crate::model::{ModelError, ToolCall};

use super::ids::random_uuid;
use super::proto::McpArgs;
use super::value::{canonicalize_json_numbers, json_from_protobuf_value};

const MCP_PROVIDER: &str = "rho";

pub(crate) fn decode_mcp_args(args: &McpArgs) -> Result<ToolCall, ModelError> {
    let mut object = Map::new();
    for (key, value) in &args.args {
        object.insert(key.clone(), decode_mcp_arg_value(value));
    }
    let name = if args.tool_name.is_empty() {
        args.name.clone()
    } else {
        args.tool_name.clone()
    };
    if is_rho_shell_tool(&name) {
        normalize_shell_timeout_args(&mut object);
    }
    let id = if args.tool_call_id.is_empty() {
        random_uuid()
    } else {
        args.tool_call_id.clone()
    };
    Ok(ToolCall {
        id,
        name,
        arguments: Value::Object(object),
    })
}

pub(crate) fn mcp_tool_definitions(
    tools: &[crate::model::ToolSpec],
) -> Vec<super::proto::McpToolDefinition> {
    use super::value::protobuf_value_from_json;
    use prost::Message;

    tools
        .iter()
        .map(|tool| super::proto::McpToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: protobuf_value_from_json(&tool.input_schema).encode_to_vec(),
            provider_identifier: MCP_PROVIDER.into(),
            tool_name: tool.name.clone(),
        })
        .collect()
}

fn is_rho_shell_tool(name: &str) -> bool {
    matches!(name, "bash" | "powershell")
}

/// Cursor-trained models send native `block_until_ms`. Persist Rho's field.
fn normalize_shell_timeout_args(object: &mut Map<String, Value>) {
    if !matches!(object.get("timeout_seconds"), None | Some(Value::Null)) {
        object.remove("block_until_ms");
        return;
    }
    if let Some(seconds) = object
        .get("block_until_ms")
        .and_then(Value::as_u64)
        .filter(|ms| *ms > 0)
        .map(|ms| ms.div_ceil(1000))
    {
        object.insert("timeout_seconds".into(), seconds.into());
    }
    object.remove("block_until_ms");
}

fn decode_mcp_arg_value(bytes: &[u8]) -> Value {
    canonicalize_json_numbers(decode_mcp_arg_value_raw(bytes))
}

fn decode_mcp_arg_value_raw(bytes: &[u8]) -> Value {
    if let Ok(parsed) = prost_types::Value::decode(bytes) {
        return json_from_protobuf_value(&parsed);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(json) = serde_json::from_str(text) {
            return json;
        }
        return Value::String(text.to_string());
    }
    Value::Null
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
