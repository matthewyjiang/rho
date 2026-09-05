use super::*;

pub(super) fn intercept(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if !request.messages.iter().any(|message| matches!(message, Message::User(blocks) if blocks.iter().any(|block| matches!(block, ContentBlock::Text(text) if text == "fixture boundary notification")))) {
        return None;
    }
    if request.messages.iter().any(|message| matches!(message, Message::ToolResult(result) if result.id.as_str() == "boundary-fixture")) {
        let delivered = request.messages.iter().any(|message| matches!(message, Message::User(blocks) if blocks.iter().any(|block| matches!(block, ContentBlock::Text(text) if text.contains("[process notification]")))));
        return Some(completed(if delivered { "background failure incorporated before completion" } else { "BUG: parent completed without background failure" }));
    }
    Some(Ok(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: "boundary-fixture".into(),
            name: "tui_fixture_progress".into(),
            arguments: serde_json::json!({"label":"boundary notification"}),
        },
    )])))
}
