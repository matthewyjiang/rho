use std::fs;
#[cfg(feature = "document-docx")]
use std::io::{Cursor, Write};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::document::{render_extracted_document, ExtractedDocument, MAX_EXTRACTED_CHARACTERS};
use crate::hashline::{compute_file_hash, format_hashline_view};
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

// Covers: ASCII image signatures that are plain text must fall back to hashline text
// Owner: read_file tool
#[tokio::test]
async fn falls_back_to_hashline_text_when_an_ascii_signature_is_not_a_decodable_image() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("fixture.txt");
    let text = "GIF89a ordinary fixture text";
    fs::write(&path, text).unwrap();

    let output = read_file_content(&path, "fixture.txt", None, None)
        .await
        .unwrap();

    assert_eq!(
        output.content,
        format_hashline_view("fixture.txt", text, None, None).unwrap()
    );
    assert!(output.image.is_none());
    assert!(output.preview_error.is_none());
}

#[tokio::test]
async fn keeps_binary_image_reads_successful_when_preview_decoding_fails() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("broken.png");
    fs::write(&path, b"\x89PNG\r\n\x1a\n\xffbroken").unwrap();

    let output = read_file_content(&path, "broken.png", None, None)
        .await
        .unwrap();

    assert_eq!(output.content, "image/png image (15 bytes)");
    assert!(output.image.is_none());
    assert!(output
        .preview_error
        .as_deref()
        .is_some_and(|error| error.starts_with("image preview unavailable:")));
}

#[cfg(feature = "document-docx")]
// Covers: whole-file read_file calls must route supported binary documents through extraction.
// Owner: read_file tool
#[tokio::test]
async fn extracts_supported_binary_documents_for_whole_file_reads() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.docx");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer
        .write_all(
            br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Tool document</w:t></w:r></w:p></w:body></w:document>"#,
        )
        .unwrap();
    fs::write(&path, writer.finish().unwrap().into_inner()).unwrap();

    let result = ReadFile
        .call(json!({"path": "sample.docx"}), ctx, "call_docx".into())
        .await
        .unwrap();

    assert_eq!(result.content, "Tool document");
}

// Covers: plain UTF-8 source reads must expose a hashline snapshot for edit
// Owner: read_file tool
#[tokio::test]
async fn plain_text_reads_return_hashline_view_with_full_file_tag() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.rs");
    let text = "fn main() {\n    ok();\n}\n";
    fs::write(&path, text).unwrap();

    let output = read_file_content(&path, "sample.rs", None, None)
        .await
        .unwrap();

    let expected = format_hashline_view("sample.rs", text, None, None).unwrap();
    assert_eq!(output.content, expected);
    assert!(output.content.contains(&compute_file_hash(text)));
    assert!(output.image.is_none());
}

// Covers: ranged plain-text reads keep absolute line numbers and the full-file tag
// Owner: read_file tool
#[tokio::test]
async fn ranged_plain_text_reads_use_hashline_window() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.txt");
    let text = "one\ntwo\nthree\nfour\n";
    fs::write(&path, text).unwrap();

    let result = ReadFile
        .call(
            json!({"path": "sample.txt", "offset": 2, "limit": 2}),
            ctx,
            "call_range".into(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.content,
        format_hashline_view("sample.txt", text, Some(2), Some(2)).unwrap()
    );
}

#[cfg(feature = "document-docx")]
// Covers: whole-file document reads must tell callers when extracted content is incomplete.
// Owner: read_file tool
#[tokio::test]
async fn reports_truncated_document_extraction() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("large.docx");
    let body = "x".repeat(MAX_EXTRACTED_CHARACTERS + 1);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    let xml = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"#
    );
    writer.write_all(xml.as_bytes()).unwrap();
    fs::write(&path, writer.finish().unwrap().into_inner()).unwrap();

    let output = read_file_content(&path, "large.docx", None, None)
        .await
        .unwrap();

    assert!(
        output
            .content
            .contains("Extraction notice: content was truncated at the output limit."),
        "{}",
        output.content
    );
    assert!(output.content.starts_with(&"x".repeat(64)));
}

#[test]
fn renders_document_warnings() {
    let content = render_extracted_document(&ExtractedDocument {
        name: "sample.docx".into(),
        mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
        text: "Document body".into(),
        truncated: false,
        warnings: vec!["archive contains ignored entries".into()],
    });

    assert_eq!(
        content,
        "Document body\n[document warning: archive contains ignored entries]"
    );
}

#[tokio::test]
async fn rejects_documents_over_the_input_limit() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("oversized.pdf");
    let file = fs::File::create(&path).unwrap();
    file.set_len(crate::document::MAX_DOCUMENT_INPUT_BYTES as u64 + 1)
        .unwrap();

    let error = match read_file_content(&path, "oversized.pdf", None, None).await {
        Ok(_) => panic!("oversized document unexpectedly succeeded"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        format!(
            "document '{}' is {} bytes; the input limit is {} bytes",
            path.display(),
            crate::document::MAX_DOCUMENT_INPUT_BYTES + 1,
            crate::document::MAX_DOCUMENT_INPUT_BYTES
        )
    );
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
