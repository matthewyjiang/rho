use pretty_assertions::assert_eq;

use super::*;
use rho_sdk::model::{ImageContent, ToolResult};

// Covers: image-bearing tool output must not replace the fixture prompt or hide
// the current turn's result. Owner: fixture request classification.
#[test]
fn image_supplements_do_not_start_user_turns() {
    let messages = vec![
        Message::user_text("previous turn"),
        Message::ToolResult(ToolResult {
            id: "old".into(),
            ok: true,
            content: "old".into(),
        }),
        Message::user_text("fixture questionnaire"),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: QUESTIONNAIRE_CALL_ID.into(),
            name: "questionnaire".into(),
            arguments: serde_json::json!({}),
        })]),
        Message::ToolResult(ToolResult {
            id: QUESTIONNAIRE_CALL_ID.into(),
            ok: true,
            content: "current answer".into(),
        }),
        Message::tool_image_supplement(
            "questionnaire",
            QUESTIONNAIRE_CALL_ID,
            vec![ImageContent {
                data: "aW1hZ2U=".into(),
                mime_type: "image/png".into(),
            }],
        )
        .unwrap(),
    ];
    let request = ModelRequest {
        messages: &messages,
        tools: &[],
        cancellation: CancellationToken::new(),
        reasoning_level: rho_sdk::ReasoningLevel::Medium,
        prompt_cache_key: None,
    };
    assert_eq!(
        fixture_response(&request).unwrap(),
        ModelResponse::Assistant(vec![ContentBlock::Text(
            "questionnaire response observed exactly 1 time(s): current answer".into()
        )])
    );
    assert_eq!(
        tool_result_for_name(&request, "questionnaire"),
        tool_result(&request, QUESTIONNAIRE_CALL_ID)
    );
    assert_eq!(
        current_turn_tool_results(&request)
            .map(|result| result.id.as_str())
            .collect::<Vec<_>>(),
        vec![QUESTIONNAIRE_CALL_ID]
    );
}

#[test]
fn compact_fixtures_match_seed_prompt_after_empty_user_entry() {
    let messages = vec![
        Message::user_text("fixture compact until cancel"),
        Message::Assistant(vec![ContentBlock::Text("ok".into())]),
        Message::User(vec![]),
    ];
    let request = ModelRequest {
        messages: &messages,
        tools: &[],
        cancellation: CancellationToken::new(),
        reasoning_level: rho_sdk::ReasoningLevel::Medium,
        prompt_cache_key: None,
    };
    assert_eq!(
        last_user_text(&request).as_deref(),
        Some("fixture compact until cancel")
    );
    assert!(compact::native_compact(request).is_some());
}
