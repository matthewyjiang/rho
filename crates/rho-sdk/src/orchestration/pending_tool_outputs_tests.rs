use pretty_assertions::assert_eq;

use crate::{model::ImageContent, tool::ToolOutput};

use super::*;

// Covers: a completed failure retains text pairing but must not deliver images.
// Owner: SDK output commitment; successful images are covered by tool_output_images.
#[test]
fn completed_failures_drop_image_supplements() {
    let completion = ToolCompletion::from_result(Ok(ToolOutput::text("result")
        .with_images(vec![ImageContent {
            data: "aW1hZ2U=".into(),
            mime_type: "image/png".into(),
        }])
        .failed()));
    let mut history = Vec::new();
    CompletedToolOutput::commit_all(
        [CompletedToolOutput::new("tool", "call", &completion)],
        &mut history,
    );
    assert_eq!(
        history,
        vec![Message::ToolResult(ToolResult {
            id: "call".into(),
            ok: false,
            content: "result".into(),
        })]
    );
}
