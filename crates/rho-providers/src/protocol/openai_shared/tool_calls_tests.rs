use pretty_assertions::assert_eq;
use serde_json::json;

use super::{finalize_chat_tool_calls, ChatToolCallPolicy, RawChatToolCall};
use crate::model::ModelError;
use rho_sdk::model::ToolCall;

// Covers: strict policy rejects empty sparse slots and missing ids
// Owner: openai chat tool-call normalization
#[test]
fn strict_policy_rejects_empty_sparse_slots() {
    let err = finalize_chat_tool_calls(
        vec![
            RawChatToolCall::default(),
            RawChatToolCall {
                id: Some("call-1".into()),
                name: Some("bash".into()),
                arguments: "{}".into(),
            },
        ],
        ChatToolCallPolicy::Strict,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ModelError::InvalidResponse(message) if message == "tool call 0 missing id"
    ));
}

// Covers: lenient policy synthesizes ids, skips holes, suffixes duplicates, coerces empty args
// Owner: openai chat tool-call normalization
#[test]
fn lenient_policy_normalizes_qwen_style_quirks() {
    let calls = finalize_chat_tool_calls(
        vec![
            RawChatToolCall::default(),
            RawChatToolCall {
                id: None,
                name: Some("bash".into()),
                arguments: String::new(),
            },
            RawChatToolCall {
                id: Some("dup".into()),
                name: Some("read_file".into()),
                arguments: r#"{"path":"a.rs"}"#.into(),
            },
            RawChatToolCall {
                id: Some("dup".into()),
                name: Some("bash".into()),
                arguments: r#"{"command":"pwd"}"#.into(),
            },
        ],
        ChatToolCallPolicy::Lenient,
    )
    .unwrap();
    assert_eq!(
        calls,
        vec![
            ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: json!({}),
            },
            ToolCall {
                id: "dup".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            },
            ToolCall {
                id: "dup_2".into(),
                name: "bash".into(),
                arguments: json!({"command": "pwd"}),
            },
        ]
    );
}
