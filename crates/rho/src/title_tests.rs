use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, ImageContent};

use super::{sanitize_title, session_input};

// Covers: title-model prose is rejected rather than displayed as a truncated title.
// Owner: title sanitizer
#[test]
fn sanitize_title_cleans_labels_and_rejects_prose() {
    let at_character_limit = "a".repeat(80);
    let over_character_limit = "a".repeat(81);
    for (input, expected) in [
        ("  \"Review the auth path\".  ", Some("Review the auth path")),
        ("\"Implement resume picker.\"", Some("Implement resume picker")),
        ("\n\n# Draft\n", Some("Draft")),
        ("   ", None),
        ("Fix one two three four five six", Some("Fix one two three four five six")),
        ("Fix one two three four five six seven", None),
        ("Unable to inspect or modify the repository because no shell or file tools are available", None),
        ("Implement resume picker\nI will inspect the repository first", None),
        (at_character_limit.as_str(), Some(at_character_limit.as_str())),
        (over_character_limit.as_str(), None),
        ("Fix \u{1b}]0;owned\u{7} the build", None),
    ] {
        assert_eq!(sanitize_title(input).as_deref(), expected, "input: {input:?}");
    }
}

// Covers: the title agent is given the images of a session's first message
// beside its text, which arrives JSON-quoted so the model reads it as
// material and not as instructions; a message of images alone gave it
// nothing to name, and the session stayed untitled.
// Owner: title agent input
#[test]
fn a_session_is_named_from_its_first_messages_text_and_images() {
    let image = ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    };
    for (label, message, source) in [
        (
            "text and image",
            vec![
                ContentBlock::Text("what is \"this\"?".into()),
                ContentBlock::Image(image.clone()),
            ],
            "First message:\n\nwhat is \"this\"?",
        ),
        (
            "image alone",
            vec![ContentBlock::Image(image.clone())],
            "First message:\n\n",
        ),
    ] {
        let input = session_input(&message);
        // The instruction line, then the material as one JSON string.
        let material: Vec<String> = input
            .blocks()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => text.split_once('\n'),
                ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
            })
            .map(|(_, quoted)| serde_json::from_str(quoted).unwrap())
            .collect();
        let images: Vec<_> = input
            .blocks()
            .iter()
            .filter(|block| matches!(block, ContentBlock::Image(_)))
            .cloned()
            .collect();
        assert_eq!(
            (material, images),
            (
                vec![source.to_owned()],
                vec![ContentBlock::Image(image.clone())]
            ),
            "{label}"
        );
    }
}
