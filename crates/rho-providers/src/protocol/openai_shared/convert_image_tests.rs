use pretty_assertions::assert_eq;

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

// Covers: assistant images must leave a visible trace instead of disappearing from history.
// Owner: OpenAI protocol wire conversion.
#[test]
fn openai_converters_replace_assistant_image_history_with_placeholder() {
    let expected = format!("generated image\n{ASSISTANT_IMAGE_OMITTED_TEXT}");

    let responses = codex_input_items(&[assistant_image()], &mut Vec::new()).unwrap();
    let chat = to_openai_message_for_target(&assistant_image(), None).unwrap();

    assert_eq!(
        responses,
        vec![json!({ "role": "assistant", "content": expected })]
    );
    assert_eq!(chat.content, Some(json!(expected)));
}
