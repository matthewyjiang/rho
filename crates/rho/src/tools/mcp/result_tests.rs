use base64::Engine;
use pretty_assertions::assert_eq;
use rho_sdk::tool::ToolErrorKind;
use rmcp::model::{CallToolResult, ContentBlock, Resource, ResourceContents};

use super::{render, RenderedResult, ResultExpectation, MAX_RETAINED_IMAGE_BYTES};

const LIMIT: usize = 12_000;

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn result(content: Vec<ContentBlock>) -> CallToolResult {
    CallToolResult::success(content)
}

fn expect_schema(schema: serde_json::Value) -> ResultExpectation {
    ResultExpectation {
        output_schema: Some(schema),
    }
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
        &ResultExpectation::default(),
        LIMIT,
    )
    .unwrap();

    assert_eq!(
        rendered.text,
        "first line\n\n\
         [image image/png, 8 B]\n\n\
         [audio audio/wav, 2.0 KB]\n\n\
         [resource file:///notes.md]\ninline body\n\n\
         [resource file:///logo.png] [resource image/png, 8 B] [not shown: card keeps only the first image]\n\n\
         [resource link file:///README.md \"readme\"]"
    );
    // The card shows one image, so only the first image block is retained.
    assert_eq!(
        rendered
            .assets
            .iter()
            .map(|asset| (asset.media_type().to_string(), asset.bytes().len()))
            .collect::<Vec<_>>(),
        vec![("image/png".to_string(), 8)]
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
        render(&mirrored, &ResultExpectation::default(), LIMIT).unwrap(),
        RenderedResult {
            text: "{\n  \"count\": 2\n}".into(),
            assets: Vec::new(),
        }
    );

    let mut with_prose = result(vec![ContentBlock::text("Found 2 matches.")]);
    with_prose.structured_content = Some(structured.clone());
    assert_eq!(
        render(&with_prose, &ResultExpectation::default(), LIMIT)
            .unwrap()
            .text,
        "Found 2 matches.\n\n{\n  \"count\": 2\n}"
    );

    let missing = render(
        &result(vec![ContentBlock::text("Found 2 matches.")]),
        &expect_schema(serde_json::json!({
            "type": "object",
            "required": ["count"],
            "properties": {"count": {"type": "integer"}},
        })),
        LIMIT,
    )
    .unwrap_err();
    assert_eq!(missing.kind(), ToolErrorKind::Execution);
    assert!(missing.message().contains("no structured content"));

    let mut valid = result(Vec::new());
    valid.structured_content = Some(structured);
    assert_eq!(
        render(
            &valid,
            &expect_schema(serde_json::json!({
                "type": "object",
                "required": ["count"],
                "properties": {"count": {"type": "integer"}},
            })),
            LIMIT,
        )
        .unwrap()
        .text,
        "{\n  \"count\": 2\n}"
    );
}

// Covers: a declared output schema must reject structured content that does
// not match, instead of handing the model a malformed half-answer.
// Owner: MCP structured-result contract.
#[test]
fn structured_content_must_match_declared_output_schema() {
    let mut invalid = result(Vec::new());
    invalid.structured_content = Some(serde_json::json!({"count": "two"}));
    let error = render(
        &invalid,
        &expect_schema(serde_json::json!({
            "type": "object",
            "required": ["count"],
            "properties": {"count": {"type": "integer"}},
        })),
        LIMIT,
    )
    .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(error
        .message()
        .contains("failed the declared output schema"));
}

// Covers: an MCP error result must fail the tool with the server's own rendered
// text, and an empty result must say so instead of returning nothing.
// Owner: MCP result rendering.
#[test]
fn error_results_and_empty_results_stay_readable() {
    let mut failed = result(vec![ContentBlock::text("disk is full")]);
    failed.is_error = Some(true);
    let error = render(&failed, &ResultExpectation::default(), LIMIT).unwrap_err();
    assert_eq!(
        (error.kind(), error.message()),
        (ToolErrorKind::Execution, "disk is full")
    );

    assert_eq!(
        render(&result(Vec::new()), &ResultExpectation::default(), LIMIT)
            .unwrap()
            .text,
        "The MCP server returned no content."
    );
}

// Covers: later images and oversized images must not retain unreachable or
// unbounded binary payloads; the model still gets a size descriptor.
// Owner: MCP result rendering.
#[test]
fn image_assets_keep_only_the_first_that_fits_the_budget() {
    let small = [0x89, b'P', b'N', b'G', 0, 1, 2, 3];
    let oversized = vec![0u8; MAX_RETAINED_IMAGE_BYTES + 1];

    let multi = render(
        &result(vec![
            ContentBlock::image(encode(&small), "image/png"),
            ContentBlock::image(encode(&small), "image/png"),
        ]),
        &ResultExpectation::default(),
        LIMIT,
    )
    .unwrap();
    assert_eq!(multi.assets.len(), 1);
    assert!(multi
        .text
        .contains("[not shown: card keeps only the first image]"));

    let too_large = render(
        &result(vec![ContentBlock::image(encode(&oversized), "image/png")]),
        &ResultExpectation::default(),
        LIMIT,
    )
    .unwrap();
    assert!(too_large.assets.is_empty());
    assert!(too_large.text.contains("not retained: exceeds"));
    assert!(!too_large.text.contains(&encode(&oversized[..16])));
}

// Covers: untrusted output schemas and structured payloads must be rejected
// before compile/validate work can monopolize CPU or memory.
// Owner: MCP structured-result contract.
#[test]
fn structured_validation_rejects_oversize_schemas() {
    // A schema with far more nodes than the budget, built without a huge
    // serialized form by nesting many tiny properties.
    let mut properties = serde_json::Map::new();
    for index in 0..3_000 {
        properties.insert(format!("f{index}"), serde_json::json!({"type": "string"}));
    }
    let schema = serde_json::json!({
        "type": "object",
        "properties": properties,
    });
    let mut call = result(Vec::new());
    call.structured_content = Some(serde_json::json!({}));
    let error = render(&call, &expect_schema(schema), LIMIT).unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(
        error.message().contains("validation budget"),
        "{}",
        error.message()
    );
}
