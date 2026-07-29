use super::*;
use crate::model::{ContentBlock, ImageContent, Message};
use pretty_assertions::assert_eq;

fn request_with_invalid_image<'a>(messages: &'a [Message]) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools: &[],
        cancellation: Default::default(),
        reasoning_level: Default::default(),
        prompt_cache_key: None,
    }
}

// Covers: only Responses Lite must replace unsafe image data before upload.
// Owner: OpenAI Responses request mode wiring.
#[tokio::test]
async fn responses_lite_applies_image_safety_policy() {
    let messages = [Message::User(vec![ContentBlock::Image(ImageContent {
        data: "not base64".into(),
        mime_type: "image/png".into(),
    })])];

    let lite = build_codex_responses_body("gpt-5.6-sol", request_with_invalid_image(&messages))
        .await
        .unwrap();
    let standard = build_codex_responses_body("gpt-5.5", request_with_invalid_image(&messages))
        .await
        .unwrap();

    assert_eq!(
        lite["input"][1]["content"],
        json!([{
            "type": "input_text",
            "text": "image content omitted because it could not be processed",
        }])
    );
    assert_eq!(
        standard["input"][0]["content"],
        json!([{
            "type": "input_image",
            "image_url": "data:image/png;base64,not base64",
        }])
    );
}
