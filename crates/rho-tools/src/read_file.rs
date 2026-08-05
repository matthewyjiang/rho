use std::{io::Cursor, path::Path};

use image::{ImageFormat, ImageReader, Limits};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::{
    document::{
        extract_document_from_bytes_async, render_extracted_document, MAX_DOCUMENT_INPUT_BYTES,
    },
    image_format::{supported_image_mime_type, MAX_IMAGE_FILE_BYTES},
    tool::*,
};

pub struct ReadFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "Reads a UTF-8 text/source file, extracts text from PDF, DOCX, XLSX, XLS, or ODS documents, or reads a PNG, JPEG, GIF, or WebP image. Text and source files always return a hashline view: a [path#TAG] header plus N:line rows. TAG fingerprints the full file snapshot (trailing whitespace ignored) so line-anchored edit can validate anchors. offset and limit select line ranges for UTF-8 text files only.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 1},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let display_path = compact_display_path(&ctx.cwd, &args.path);
        let output = read_file_content(&path, &display_path, args.offset, args.limit).await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(output.content, ctx.max_output_bytes),
        })
    }
}

pub(super) fn read_file_display_content(
    cwd: &std::path::Path,
    path: &str,
    args: &serde_json::Value,
) -> String {
    let path = compact_display_path(cwd, path);
    let offset = args
        .get("offset")
        .and_then(|offset| offset.as_u64())
        .and_then(|offset| usize::try_from(offset).ok());
    let limit = args
        .get("limit")
        .and_then(|limit| limit.as_u64())
        .and_then(|limit| usize::try_from(limit).ok());

    if offset.is_none() && limit.is_none() {
        return path;
    }

    let start = offset.unwrap_or(1);
    let end = limit
        .map(|limit| start.saturating_add(limit).saturating_sub(1).to_string())
        .unwrap_or_else(|| "end".into());
    format!("{path}:{start}-{end}")
}

const MAX_DECODE_DIMENSION: u32 = 4_096;
const MAX_DECODE_ALLOCATION: u64 = 80 * 1024 * 1024;
const THUMBNAIL_WIDTH: u32 = 1_024;
const THUMBNAIL_HEIGHT: u32 = 768;

pub(super) struct ImageAsset {
    pub(super) media_type: &'static str,
    pub(super) bytes: Vec<u8>,
}

pub(super) struct ReadFileContent {
    pub(super) content: String,
    pub(super) image: Option<ImageAsset>,
    pub(super) preview_error: Option<String>,
    /// Full file text plus 1-indexed lines shown in the hashline view, when the
    /// body was read as UTF-8 source (for session snapshot recording).
    pub(super) snapshot: Option<(String, Vec<usize>)>,
}

pub(super) async fn read_file_content(
    path: &Path,
    display_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ReadFileContent, ToolError> {
    let mut file = tokio::fs::File::open(path).await?;
    let source_len = file.metadata().await?.len();

    // Range reads are always hashline text views of the on-disk UTF-8 body. The
    // whole body is read because the header tag covers the whole file, so the
    // document size limit applies here just as it does to a full read.
    if offset.is_some() || limit.is_some() {
        check_document_size(path, source_len)?;
        let text = tokio::fs::read_to_string(path).await.map_err(|error| {
            ToolError::Message(format!(
                "could not read '{}' as UTF-8 text: {error}",
                path.display()
            ))
        })?;
        let content = crate::hashline::format_hashline_view(display_path, &text, offset, limit)
            .map_err(ToolError::Message)?;
        let seen = seen_lines_for_view(&text, offset, limit);
        return Ok(ReadFileContent {
            content,
            image: None,
            preview_error: None,
            snapshot: Some((text, seen)),
        });
    }

    let mut header = [0_u8; 12];
    let header_len = file.read(&mut header).await?;
    if let Some(mime_type) = supported_image_mime_type(&header[..header_len]) {
        let content = format!("{mime_type} image ({source_len} bytes)");
        if source_len > MAX_IMAGE_FILE_BYTES {
            return Ok(ReadFileContent {
                content,
                image: None,
                preview_error: Some(format!(
                    "image preview unavailable: file exceeds the {MAX_IMAGE_FILE_BYTES} byte preview limit"
                )),
                snapshot: None,
            });
        }

        let mut bytes = Vec::with_capacity(source_len as usize);
        bytes.extend_from_slice(&header[..header_len]);
        (&mut file)
            .take(MAX_IMAGE_FILE_BYTES + 1 - header_len as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() as u64 > MAX_IMAGE_FILE_BYTES {
            return Ok(ReadFileContent {
                content,
                image: None,
                preview_error: Some(format!(
                    "image preview unavailable: file exceeds the {MAX_IMAGE_FILE_BYTES} byte preview limit"
                )),
                snapshot: None,
            });
        }
        let content = format!("{mime_type} image ({} bytes)", bytes.len());
        let display_path = display_path.to_string();
        return match tokio::task::spawn_blocking(move || thumbnail_png(bytes)).await {
            Ok(Ok(thumbnail)) => Ok(ReadFileContent {
                content,
                image: Some(ImageAsset {
                    media_type: "image/png",
                    bytes: thumbnail,
                }),
                preview_error: None,
                snapshot: None,
            }),
            Ok(Err((error, bytes))) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let content =
                        crate::hashline::format_hashline_view(&display_path, &text, None, None)
                            .map_err(ToolError::Message)?;
                    let seen = seen_lines_for_view(&text, None, None);
                    Ok(ReadFileContent {
                        content,
                        image: None,
                        preview_error: None,
                        snapshot: Some((text, seen)),
                    })
                }
                Err(_) => Ok(ReadFileContent {
                    content,
                    image: None,
                    preview_error: Some(format!("image preview unavailable: {error}")),
                    snapshot: None,
                }),
            },
            Err(error) => Ok(ReadFileContent {
                content,
                image: None,
                preview_error: Some(format!("image preview task failed: {error}")),
                snapshot: None,
            }),
        };
    }

    check_document_size(path, source_len)?;
    let mut bytes = Vec::with_capacity(source_len as usize);
    bytes.extend_from_slice(&header[..header_len]);
    (&mut file)
        .take(MAX_DOCUMENT_INPUT_BYTES as u64 + 1 - header_len as u64)
        .read_to_end(&mut bytes)
        .await?;
    // Plain UTF-8 sources use the on-disk bytes for hashline tags so
    // edit can validate against the same snapshot. Rich documents keep
    // the extractor path and are not hashline-editable.
    if !is_rich_document_path(path, &bytes) {
        let text = String::from_utf8(bytes).map_err(|error| {
            ToolError::Message(format!(
                "file '{}' is not valid UTF-8 text: {error}",
                path.display()
            ))
        })?;
        let content = crate::hashline::format_hashline_view(display_path, &text, None, None)
            .map_err(ToolError::Message)?;
        let seen = seen_lines_for_view(&text, None, None);
        return Ok(ReadFileContent {
            content,
            image: None,
            preview_error: None,
            snapshot: Some((text, seen)),
        });
    }

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let document = extract_document_from_bytes_async(name, bytes)
        .await
        .map_err(|error| ToolError::Message(error.to_string()))?;
    Ok(ReadFileContent {
        content: render_extracted_document(&document),
        image: None,
        preview_error: None,
        snapshot: None,
    })
}

fn seen_lines_for_view(text: &str, offset: Option<usize>, limit: Option<usize>) -> Vec<usize> {
    let total = crate::hashline::split_content_lines_for_store(text);
    if total == 0 {
        return Vec::new();
    }
    let start = offset.unwrap_or(1).max(1);
    let end = match limit {
        Some(limit) => start.saturating_add(limit).saturating_sub(1).min(total),
        None => total,
    };
    if start > total {
        return Vec::new();
    }
    (start..=end).collect()
}

fn check_document_size(path: &Path, source_len: u64) -> Result<(), ToolError> {
    if source_len > MAX_DOCUMENT_INPUT_BYTES as u64 {
        return Err(ToolError::Message(format!(
            "document '{}' is {source_len} bytes; the input limit is {MAX_DOCUMENT_INPUT_BYTES} bytes",
            path.display()
        )));
    }
    Ok(())
}

fn thumbnail_png(bytes: Vec<u8>) -> Result<Vec<u8>, (image::ImageError, Vec<u8>)> {
    let result = (|| {
        let mut reader = ImageReader::new(Cursor::new(bytes.as_slice())).with_guessed_format()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_DECODE_DIMENSION);
        limits.max_image_height = Some(MAX_DECODE_DIMENSION);
        limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
        reader.limits(limits);
        let thumbnail = reader
            .decode()?
            .thumbnail(THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT);
        let mut encoded = Cursor::new(Vec::new());
        thumbnail.write_to(&mut encoded, ImageFormat::Png)?;
        Ok(encoded.into_inner())
    })();
    result.map_err(|error| (error, bytes))
}

fn is_rich_document_path(path: &Path, bytes: &[u8]) -> bool {
    if bytes.starts_with(b"%PDF-") {
        return true;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        extension.as_deref(),
        Some("pdf" | "docx" | "xlsx" | "xls" | "ods")
    )
}

#[cfg(test)]
#[path = "read_file_tests.rs"]
mod tests;
