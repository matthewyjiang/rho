use pretty_assertions::assert_eq;
use prost::Message;
use serde_json::json;

use super::decode_mcp_args;
use crate::protocol::cursor::proto::McpArgs;
use crate::protocol::cursor::value::protobuf_value_from_json;

fn bash_args(tool_call_id: &str, fields: &[(&str, serde_json::Value)]) -> McpArgs {
    let mut args = McpArgs {
        name: "bash".into(),
        tool_name: String::new(),
        tool_call_id: tool_call_id.into(),
        provider_identifier: "rho".into(),
        args: Default::default(),
    };
    for (key, value) in fields {
        args.args.insert(
            (*key).into(),
            protobuf_value_from_json(value).encode_to_vec(),
        );
    }
    args
}

// Covers: MCP exec args are protobuf Values and must become a JSON object ToolCall
// Owner: cursor protocol
#[test]
fn mcp_args_decode_protobuf_values_into_tool_call_object() {
    let mut args = McpArgs {
        name: "read_file".into(),
        tool_name: String::new(),
        tool_call_id: "call-9".into(),
        provider_identifier: "rho".into(),
        args: Default::default(),
    };
    args.args.insert(
        "path".into(),
        protobuf_value_from_json(&json!("/tmp/a.rs")).encode_to_vec(),
    );

    assert_eq!(
        decode_mcp_args(&args).unwrap(),
        crate::model::ToolCall {
            id: "call-9".into(),
            name: "read_file".into(),
            arguments: json!({ "path": "/tmp/a.rs" }),
        }
    );
}

// Covers: protobuf NumberValue is always f64; bash timeout_seconds must decode as u64
// Owner: cursor protocol
#[test]
fn mcp_args_decode_whole_numbers_as_json_integers() {
    let call = decode_mcp_args(&bash_args(
        "call-timeout",
        &[
            (
                "timeout_seconds",
                serde_json::Value::Number(serde_json::Number::from_f64(30.0).expect("finite")),
            ),
            ("command", json!("true")),
        ],
    ))
    .unwrap();
    assert_eq!(call.arguments["timeout_seconds"].as_u64(), Some(30));
    assert_eq!(call.arguments["command"], json!("true"));
}

// Covers: Cursor native block_until_ms must become timeout_seconds before bash parses
// Owner: cursor protocol
#[test]
fn mcp_shell_args_remap_block_until_ms_to_timeout_seconds() {
    let cases = [
        (
            &[
                ("command", json!("true")),
                ("block_until_ms", json!(45_000)),
            ][..],
            Some(45_u64),
        ),
        (
            &[
                ("command", json!("true")),
                (
                    "block_until_ms",
                    serde_json::Value::Number(serde_json::Number::from_f64(500.0).expect("finite")),
                ),
            ],
            Some(1),
        ),
        (
            &[
                ("command", json!("true")),
                ("timeout_seconds", json!(10)),
                ("block_until_ms", json!(45_000)),
            ],
            Some(10),
        ),
        (
            &[("command", json!("true")), ("block_until_ms", json!(0))],
            None,
        ),
    ];

    for (fields, expected) in cases {
        let call = decode_mcp_args(&bash_args("call-alias", fields)).unwrap();
        assert_eq!(
            call.arguments
                .get("timeout_seconds")
                .and_then(serde_json::Value::as_u64),
            expected,
            "fields={fields:?}"
        );
        assert!(call.arguments.get("block_until_ms").is_none());
    }
}

// Covers: missing MCP tool_call_id must not collide across two calls of the same tool
// Owner: cursor protocol
#[test]
fn missing_mcp_tool_call_id_is_unique_per_call() {
    let first = decode_mcp_args(&bash_args("", &[("command", json!("true"))])).unwrap();
    let second = decode_mcp_args(&bash_args("", &[("command", json!("true"))])).unwrap();
    assert_ne!(first.id, second.id);
    assert!(!first.id.is_empty());
    assert!(!second.id.is_empty());
}
