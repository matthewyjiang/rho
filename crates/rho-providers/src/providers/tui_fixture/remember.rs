//! Verify that locally remembered instructions reach the next provider request.

use rho_sdk::{
    model::{ContentBlock, ModelRequest, ModelResponse, SemanticMessage},
    ProviderError,
};

use super::completed;

pub(super) fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt != "fixture remembered context" {
        return None;
    }
    let last_user = request
        .messages
        .iter()
        .rposition(|message| matches!(message.semantic(), SemanticMessage::User(_)))?;
    let present = request.messages[..last_user].iter().any(|message| {
        let SemanticMessage::User(content) = message.semantic() else {
            return false;
        };
        content.iter().any(
            |block| matches!(block, ContentBlock::Text(text) if text.contains("use max jobs 8")),
        )
    });
    Some(completed(if present {
        "remembered context present"
    } else {
        "remembered context missing"
    }))
}
