use std::path::Path;

use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader};

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
            description: "Reads a UTF-8 text file or a PNG, JPEG, GIF, or WebP image.".into(),
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
        let output = read_file_content(&path, args.offset, args.limit).await?;
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

pub(super) struct ReadFileContent {
    pub(super) content: String,
    pub(super) image_mime_type: Option<&'static str>,
}

pub(super) async fn read_file_content(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ReadFileContent, ToolError> {
    if offset.is_none() && limit.is_none() {
        let mut file = tokio::fs::File::open(path).await?;
        let mut header = [0_u8; 12];
        let header_len = file.read(&mut header).await?;
        if let Some(mime_type) = supported_image_mime_type(&header[..header_len]) {
            let bytes = file.metadata().await?.len();
            return Ok(ReadFileContent {
                content: format!("{mime_type} image ({bytes} bytes)"),
                image_mime_type: Some(mime_type),
            });
        }
        return Ok(ReadFileContent {
            content: tokio::fs::read_to_string(path).await?,
            image_mime_type: None,
        });
    }
    let file = tokio::fs::File::open(path).await?;
    Ok(ReadFileContent {
        content: read_line_range(BufReader::new(file), offset, limit).await?,
        image_mime_type: None,
    })
}

fn supported_image_mime_type(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if header.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

async fn read_line_range(
    mut reader: impl AsyncBufRead + Unpin,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    if offset == Some(0) {
        return Err(ToolError::Message("offset must be greater than 0".into()));
    }
    if limit == Some(0) {
        return Err(ToolError::Message("limit must be greater than 0".into()));
    }

    let start = offset.unwrap_or(1) - 1;
    let mut line_number = 0;
    let mut selected_lines = 0;
    let mut selected = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            if start > 0 && start >= line_number {
                return Err(ToolError::Message(format!(
                    "offset {} is past the end of the file ({line_number} line(s))",
                    start + 1
                )));
            }
            return Ok(selected);
        }
        line_number += 1;
        if line_number <= start {
            continue;
        }
        selected.push_str(&line);
        selected_lines += 1;
        if limit == Some(selected_lines) {
            return Ok(selected);
        }
    }
}

#[cfg(test)]
#[path = "read_file_tests.rs"]
mod tests;
