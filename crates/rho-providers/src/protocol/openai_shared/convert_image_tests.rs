use pretty_assertions::assert_eq;
use serde_json::json;

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

fn xai_identity() -> crate::model::ModelIdentity {
    crate::model::ModelIdentity::new("xai", "openai-responses", "grok-4.5")
}

fn assistant_image_with_slim_replay() -> Message {
    let identity = xai_identity();
    Message::assistant(crate::model::AssistantMessage {
        content: vec![
            ContentBlock::Text("generated image".into()),
            ContentBlock::Image(ImageContent {
                data: "aW1hZ2U=".into(),
                mime_type: "image/png".into(),
            }),
        ],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: vec![crate::model::ProviderContextBlock {
            identity,
            kind: "openai_response_output_item".into(),
            position: Some(0),
            data: json!({
                "type": "image_generation_call",
                "id": "ig_1",
                "status": "completed",
                "prompt": "a corgi",
            }),
        }],
    })
}

// Covers: same-provider image_generation_call replay must carry the image
// result and must not also send the omitted-image placeholder.
// Owner: OpenAI protocol wire conversion.
#[test]
fn responses_replay_restores_image_result_without_placeholder() {
    let mut instructions = Vec::new();
    let target = xai_identity();
    let items = codex_input_items_for_target(
        &[assistant_image_with_slim_replay()],
        &mut instructions,
        Some(&target),
    )
    .unwrap();

    assert_eq!(
        items,
        vec![
            json!({
                "type": "image_generation_call",
                "id": "ig_1",
                "status": "completed",
                "prompt": "a corgi",
                "result": "aW1hZ2U=",
            }),
            json!({ "role": "assistant", "content": "generated image" }),
        ]
    );
}
