use super::*;
use crate::model::ImageContent;

fn assistant_image() -> Message {
    Message::Assistant(vec![
        ContentBlock::Text("generated image".into()),
        ContentBlock::Image(ImageContent {
            data: "aW1hZ2U=".into(),
            mime_type: "image/png".into(),
        }),
    ])
}

// Covers: assistant images must fail clearly instead of disappearing from history.
// Owner: OpenAI protocol wire conversion.
#[test]
fn openai_converters_reject_unsupported_assistant_image_history() {
    let responses_error = codex_input_items(vec![assistant_image()], &mut Vec::new()).unwrap_err();
    let Err(chat_error) = to_openai_message_for_target(assistant_image(), None) else {
        panic!("assistant image history was silently accepted")
    };

    assert!(matches!(
        responses_error,
        ModelError::InvalidResponse(message)
            if message == "OpenAI Responses cannot encode image content in assistant history"
    ));
    assert!(matches!(
        chat_error,
        ModelError::InvalidResponse(message)
            if message == "OpenAI Chat Completions cannot encode image content in assistant history"
    ));
}
