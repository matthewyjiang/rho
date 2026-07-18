use std::fs;

use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::*;

fn test_context() -> (TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 12000,
    };
    (dir, ctx)
}

#[tokio::test]
async fn reads_selected_line_range() {
    let (_dir, ctx) = test_context();
    fs::write(ctx.cwd.join("sample.txt"), "one\ntwo\nthree\nfour\n").unwrap();

    let result = ReadFile
        .call(
            json!({"path": "sample.txt", "offset": 2, "limit": 2}),
            ctx,
            "call_1".into(),
        )
        .await
        .unwrap();

    assert_eq!(result.content, "two\nthree\n");
}

#[tokio::test]
async fn reads_supported_images_without_retaining_the_source_decode() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("photo.png");
    image::RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255]))
        .save(&path)
        .unwrap();
    let source_len = fs::metadata(path).unwrap().len();

    let result = ReadFile
        .call(json!({"path": "photo.png"}), ctx, "call_image".into())
        .await
        .unwrap();

    assert_eq!(
        result.content,
        format!("image/png image ({source_len} bytes)")
    );
}

#[tokio::test]
async fn keeps_image_reads_successful_when_preview_decoding_fails() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("fixture.gif");
    fs::write(&path, "GIF89a ordinary fixture text").unwrap();

    let output = read_file_content(&path, None, None).await.unwrap();

    assert_eq!(output.content, "image/gif image (28 bytes)");
    assert!(output.image.is_none());
    assert!(output
        .preview_error
        .as_deref()
        .is_some_and(|error| error.starts_with("image preview unavailable:")));
}

#[test]
fn recognizes_supported_image_signatures() {
    assert_eq!(
        supported_image_mime_type(b"\xff\xd8\xffrest"),
        Some("image/jpeg")
    );
    assert_eq!(supported_image_mime_type(b"GIF89arest"), Some("image/gif"));
    assert_eq!(
        supported_image_mime_type(b"RIFFxxxxWEBP"),
        Some("image/webp")
    );
    assert_eq!(supported_image_mime_type(b"plain text"), None);
}

#[tokio::test]
async fn rejects_offset_past_end_of_file() {
    let (_dir, ctx) = test_context();
    fs::write(ctx.cwd.join("sample.txt"), "one\ntwo\n").unwrap();

    let err = ReadFile
        .call(
            json!({"path": "sample.txt", "offset": 5}),
            ctx,
            "call_1".into(),
        )
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "offset 5 is past the end of the file (2 line(s))"
    );
}

#[tokio::test]
async fn rejects_zero_offset() {
    let (_dir, ctx) = test_context();
    fs::write(ctx.cwd.join("sample.txt"), "one\n").unwrap();

    let err = ReadFile
        .call(
            json!({"path": "sample.txt", "offset": 0}),
            ctx,
            "call_1".into(),
        )
        .await
        .unwrap_err();

    assert_eq!(err.to_string(), "offset must be greater than 0");
}

#[tokio::test]
async fn rejects_zero_limit() {
    let (_dir, ctx) = test_context();
    fs::write(ctx.cwd.join("sample.txt"), "one\n").unwrap();

    let err = ReadFile
        .call(
            json!({"path": "sample.txt", "limit": 0}),
            ctx,
            "call_1".into(),
        )
        .await
        .unwrap_err();

    assert_eq!(err.to_string(), "limit must be greater than 0");
}

#[tokio::test]
async fn ranged_read_stops_after_limit() {
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, ReadBuf};

    struct FailsAfterPrefix {
        prefix: &'static [u8],
        position: usize,
    }

    impl AsyncRead for FailsAfterPrefix {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.position == self.prefix.len() {
                return Poll::Ready(Err(io::Error::other("read past requested range")));
            }
            let remaining = &self.prefix[self.position..];
            let length = remaining.len().min(buffer.remaining());
            buffer.put_slice(&remaining[..length]);
            self.position += length;
            Poll::Ready(Ok(()))
        }
    }

    let reader = BufReader::with_capacity(
        1,
        FailsAfterPrefix {
            prefix: b"one\ntwo\n",
            position: 0,
        },
    );
    let content = read_line_range(reader, Some(1), Some(2)).await.unwrap();

    assert_eq!(content, "one\ntwo\n");
}
