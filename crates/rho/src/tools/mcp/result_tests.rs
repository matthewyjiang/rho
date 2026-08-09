use base64::Engine;
use pretty_assertions::assert_eq;
use rho_sdk::tool::ToolErrorKind;
use rmcp::model::{CallToolResult, ContentBlock, Resource, ResourceContents};

use super::{render, RenderedResult, ResultExpectation};

const LIMIT: usize = 12_000;

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn result(content: Vec<ContentBlock>) -> CallToolResult {
    CallToolResult::success(content)
}

// Covers: each content kind must reach the model as something it can act on.
// Text stays text, binary becomes a short descriptor plus a card asset rather
// than a base64 wall, and resource blocks name their URI.
// Owner: MCP result rendering.
#[test]
fn content_blocks_render_by_kind() {
    let png = [0x89, b'P', b'N', b'G', 0, 1, 2, 3];
    let rendered = render(
        &result(vec![
            ContentBlock::text("first line"),
            ContentBlock::image(encode(&png), "image/png"),
            ContentBlock::audio(encode(&[0u8; 2048]), "audio/wav"),
            ContentBlock::resource(ResourceContents::text("inline body", "file:///notes.md")),
            ContentBlock::resource(ResourceContents::BlobResourceContents {
                uri: "file:///logo.png".into(),
                mime_type: Some("image/png".into()),
                blob: encode(&png),
                meta: None,
            }),
            ContentBlock::ResourceLink(Resource::new("file:///README.md", "readme")),
        ]),
        ResultExpectation::default(),
        LIMIT,
    )
    .unwrap();

    assert_eq!(
        rendered.text,
        "first line\n\n\
         [image image/png, 8 B]\n\n\
         [audio audio/wav, 2.0 KB]\n\n\
         [resource file:///notes.md]\ninline body\n\n\
         [resource file:///logo.png] [resource image/png, 8 B]\n\n\
         [resource link file:///README.md \"readme\"]"
    );
    // Only the images become renderable assets; audio has no card renderer.
    assert_eq!(
        rendered
            .assets
            .iter()
            .map(|asset| (asset.media_type().to_string(), asset.bytes().len()))
            .collect::<Vec<_>>(),
        vec![("image/png".to_string(), 8), ("image/png".to_string(), 8)]
    );
}

// Covers: structured content must be presented once. A server that mirrors it
// as text for older clients must not cost the context twice, and a server that
// declares an output schema but returns nothing structured must fail loudly.
// Owner: MCP structured-result contract.
#[test]
fn structured_content_is_presented_once_and_required_when_declared() {
    let structured = serde_json::json!({"count": 2});
    let mut mirrored = result(vec![ContentBlock::text(structured.to_string())]);
    mirrored.structured_content = Some(structured.clone());

    assert_eq!(
        render(&mirrored, ResultExpectation::default(), LIMIT).unwrap(),
        RenderedResult {
            text: "{\n  \"count\": 2\n}".into(),
            assets: Vec::new(),
        }
    );

    let mut with_prose = result(vec![ContentBlock::text("Found 2 matches.")]);
    with_prose.structured_content = Some(structured);
    assert_eq!(
        render(&with_prose, ResultExpectation::default(), LIMIT)
            .unwrap()
            .text,
        "Found 2 matches.\n\n{\n  \"count\": 2\n}"
    );

    let missing = render(
        &result(vec![ContentBlock::text("Found 2 matches.")]),
        ResultExpectation {
            structured_content: true,
        },
        LIMIT,
    )
    .unwrap_err();
    assert_eq!(missing.kind(), ToolErrorKind::Execution);
    assert!(missing.message().contains("no structured content"));
}

// Covers: an MCP error result must fail the tool with the server's own rendered
// text, and an empty result must say so instead of returning nothing.
// Owner: MCP result rendering.
#[test]
fn error_results_and_empty_results_stay_readable() {
    let mut failed = result(vec![ContentBlock::text("disk is full")]);
    failed.is_error = Some(true);
    let error = render(&failed, ResultExpectation::default(), LIMIT).unwrap_err();
    assert_eq!(
        (error.kind(), error.message()),
        (ToolErrorKind::Execution, "disk is full")
    );

    assert_eq!(
        render(&result(Vec::new()), ResultExpectation::default(), LIMIT)
            .unwrap()
            .text,
        "The MCP server returned no content."
    );
}
