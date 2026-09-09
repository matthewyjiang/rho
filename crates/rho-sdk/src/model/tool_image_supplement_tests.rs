use pretty_assertions::assert_eq;

use super::super::SemanticMessage;
use super::*;

// Covers: history classification must recognize exact tool attribution without
// treating ordinary human image uploads or malformed mixed content as supplements.
// Owner: SDK compatibility encoding.
#[test]
fn recognize_tool_images_without_classifying_human_uploads() {
    let image = ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    };
    let valid =
        Message::tool_image_supplement("capture\"screen", "call-1", vec![image.clone()]).unwrap();
    let round_trip =
        serde_json::from_str::<Message>(&serde_json::to_string(&valid).unwrap()).unwrap();
    let Message::User(mut missing_image) = valid.clone() else {
        unreachable!()
    };
    missing_image.pop();
    let Message::User(mut mixed) = valid.clone() else {
        unreachable!()
    };
    mixed.push(ContentBlock::Text("human instruction".into()));
    let Message::User(mut malformed) = valid.clone() else {
        unreachable!()
    };
    malformed[0] = ContentBlock::Text(format!("{PREFIX}{{}}{SUFFIX}"));
    for (message, expected) in [
        (valid, true),
        (round_trip, true),
        (
            Message::User(vec![
                ContentBlock::Text("describe this".into()),
                ContentBlock::Image(image.clone()),
            ]),
            false,
        ),
        (Message::User(missing_image), false),
        (Message::User(mixed), false),
        (Message::User(malformed), false),
        (Message::assistant_text("capture"), false),
    ] {
        let recognized = match message.semantic() {
            SemanticMessage::ToolImageSupplement(images) => Some(images),
            SemanticMessage::User(_) | SemanticMessage::Assistant(_) => None,
            other => panic!("unexpected classification: {other:?}"),
        };
        assert_eq!(recognized.is_some(), expected, "{message:?}");
        assert_eq!(message.as_tool_image_supplement().is_some(), expected);
        if let Some(supplement) = recognized {
            assert_eq!(
                (
                    supplement.tool_name(),
                    supplement.tool_call_id(),
                    supplement.images().collect::<Vec<_>>()
                ),
                ("capture\"screen", "call-1", vec![&image]),
            );
        }
    }
    assert_eq!(
        Message::tool_image_supplement("capture", "call-1", vec![]),
        None
    );
}
