use pretty_assertions::assert_eq;

use super::{super::tests::test_app, resource_attachment, McpResource, McpResourceContent};
use crate::tui::media_attach::MediaAttachOutcome;

const LARGE_CAP: usize = 64 * 1024;
/// One-pixel PNG, exactly as a server would send it.
const PNG_BLOB: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn text(uri: &str, mime: Option<&str>, body: &str) -> McpResourceContent {
    McpResourceContent::Text {
        uri: uri.into(),
        mime_type: mime.map(str::to_string),
        text: body.into(),
    }
}

fn blob(uri: &str, mime: Option<&str>, data: &str) -> McpResourceContent {
    McpResourceContent::Blob {
        uri: uri.into(),
        mime_type: mime.map(str::to_string),
        blob: data.into(),
    }
}

/// What a finished read turned into, flattened so a whole case compares at once.
#[derive(Debug, PartialEq, Eq)]
enum Attached {
    Image {
        data: String,
        mime_type: String,
    },
    Document {
        mime: String,
        body: String,
        truncated: bool,
    },
}

fn attached(outcome: MediaAttachOutcome) -> Attached {
    match outcome {
        MediaAttachOutcome::Ready {
            media: super::ChatMedia::Image(image),
            ..
        } => Attached::Image {
            data: image.data,
            mime_type: image.mime_type,
        },
        MediaAttachOutcome::Ready {
            media: super::ChatMedia::TextDocument(document),
            ..
        } => Attached::Document {
            mime: document.mime,
            body: document.body,
            truncated: document.truncated,
        },
        MediaAttachOutcome::Unsupported { .. } | MediaAttachOutcome::Failed { .. } => {
            panic!("a successful read must produce media")
        }
    }
}

// Covers: read bodies must become the right attachment kind, and an image blob
// must reach the provider as the exact base64 the server sent, with no decode
// and re-encode round trip that could alter or inflate it.
// Owner: pure unit (MCP resource to composer media mapping).
#[test]
fn resource_contents_map_onto_composer_media() {
    let cases = [
        (
            "text keeps its body and declared type",
            vec![text("file:///notes.md", Some("text/markdown"), "hello")],
            LARGE_CAP,
            Attached::Document {
                mime: "text/markdown".into(),
                body: "hello".into(),
                truncated: false,
            },
        ),
        (
            "text without a declared type falls back to plain text",
            vec![text("file:///notes", None, "hello")],
            LARGE_CAP,
            Attached::Document {
                mime: "text/plain".into(),
                body: "hello".into(),
                truncated: false,
            },
        ),
        (
            "a lone image blob is attached as an image, byte for byte",
            vec![blob("file:///pixel.png", Some("image/png"), PNG_BLOB)],
            LARGE_CAP,
            Attached::Image {
                data: PNG_BLOB.into(),
                mime_type: "image/png".into(),
            },
        ),
        (
            "other binary is described rather than inlined",
            vec![blob("db://report", Some("application/pdf"), "cGRmYnl0ZXM=")],
            LARGE_CAP,
            Attached::Document {
                mime: "application/pdf".into(),
                body: "[resource application/pdf, 8 B]".into(),
                truncated: false,
            },
        ),
        (
            "a multi-part read arrives whole instead of losing parts",
            vec![
                text("file:///a", Some("text/plain"), "first"),
                blob("file:///b", Some("image/png"), PNG_BLOB),
            ],
            LARGE_CAP,
            Attached::Document {
                mime: "application/octet-stream".into(),
                body: "first\n\n[resource image/png, 70 B]".into(),
                truncated: false,
            },
        ),
        (
            "a body from a newer spec revision is named, not dropped",
            vec![McpResourceContent::Unsupported],
            LARGE_CAP,
            Attached::Document {
                mime: "application/octet-stream".into(),
                body: "[resource of a kind Rho does not render]".into(),
                truncated: false,
            },
        ),
        (
            "an oversized body is capped and says so",
            vec![text("file:///big", Some("text/plain"), "abcdefghij")],
            4,
            Attached::Document {
                mime: "text/plain".into(),
                body: "abcd\n[truncated]".into(),
                truncated: true,
            },
        ),
    ];

    for (name, contents, max_output_bytes, expected) in cases {
        assert_eq!(
            attached(resource_attachment("res://x", &contents, max_output_bytes)),
            expected,
            "{name}"
        );
    }
}

// Covers: a lone image resource larger than the paste attachment ceiling must
// be refused rather than cloned into the composer and provider request.
// Owner: pure unit (MCP resource to composer media mapping).
#[test]
fn oversized_image_resource_is_rejected() {
    use base64::Engine;

    let bytes = vec![0u8; (rho_tools::MAX_IMAGE_FILE_BYTES as usize) + 1];
    let blob = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let outcome = resource_attachment(
        "res://huge.png",
        &[McpResourceContent::Blob {
            uri: "res://huge.png".into(),
            mime_type: Some("image/png".into()),
            blob,
        }],
        LARGE_CAP,
    );
    match outcome {
        MediaAttachOutcome::Failed { kind, message } => {
            assert_eq!(kind, "resource read");
            assert!(message.contains("attachment limit"), "{message}");
        }
        MediaAttachOutcome::Ready { .. } => panic!("expected failure, got Ready"),
        MediaAttachOutcome::Unsupported { .. } => panic!("expected failure, got Unsupported"),
    }
}

// Covers: a resources/read that fails must take its pending entry with it.
// Submission is gated on pending attachments, so a failed read that left the
// entry behind would lock the composer with no way out.
// Owner: TUI composer attachment orchestration.
//
// Not a PTY scenario: driving a real failure through the terminal needs a live
// MCP server, and the harness has no MCP server fixture.
#[tokio::test]
async fn failed_resource_read_clears_its_pending_attachment() {
    let mut app = test_app();
    app.insert_pasted_input_text("look at @res://missing");
    // No server is connected, so the read cannot be served and the task fails
    // on its own. Nothing here waits on the clock.
    app.start_mcp_resource_attach(&McpResource {
        server: "absent".into(),
        uri: "res://missing".into(),
        name: "missing".into(),
        title: None,
        description: None,
        mime_type: None,
        templated: false,
    })
    .unwrap();

    assert_eq!(app.input_ui.pending_attachment_count(), 1);

    let completion =
        crate::tui::media_attach::next_media_attach_completion(&mut app.media_attach_tasks).await;
    app.finish_media_attach(completion);

    assert_eq!(app.input_ui.attachments(), &[]);
    assert!(app.media_attach_tasks.is_empty());
    assert_eq!(
        app.status(),
        "resource read failed: no connected MCP server named `absent`"
    );
}
