use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::protocol::openai_chat::ChatToolCallPolicy;

// Covers: custom hosts and Qwen must keep accepting omitted tool-call ids
// Owner: openai-compatible dialect
#[test]
fn chat_tool_call_policy_opts_unknown_hosts_into_lenient() {
    assert_eq!(
        OpenAiCompatibleDialect::Custom.chat_tool_call_policy(),
        ChatToolCallPolicy::Lenient
    );
    assert_eq!(
        OpenAiCompatibleDialect::QwenTokenPlan.chat_tool_call_policy(),
        ChatToolCallPolicy::Lenient
    );
    assert_eq!(
        OpenAiCompatibleDialect::Standard.chat_tool_call_policy(),
        ChatToolCallPolicy::Strict
    );
}

#[test]
fn moonshot_parameters_keep_required_root_object_type() {
    let mut schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "anyOf": [
            {"type": "object", "required": ["path"]},
            {"type": "object", "required": ["edits"]}
        ]
    });

    normalize_moonshot_parameters(&mut schema);

    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}}
        })
    );
}

#[test]
fn moonshot_moves_parent_type_into_any_of_branches() {
    let mut schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "anyOf": [
            {"type": "object", "required": ["path"]},
            {"required": ["edits"]}
        ]
    });

    normalize_moonshot_schema(&mut schema);

    assert_eq!(
        schema,
        json!({
            "properties": {"path": {"type": "string"}},
            "anyOf": [
                {"type": "object", "required": ["path"]},
                {"type": "object", "required": ["edits"]}
            ]
        })
    );
}
