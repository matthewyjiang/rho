use std::path::Path;

use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};

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
            description: "Reads a UTF-8 text file.".into(),
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
        let content = read_file_content(&path, args.offset, args.limit).await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(content, ctx.max_output_bytes),
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

pub(super) async fn read_file_content(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    if offset.is_none() && limit.is_none() {
        return Ok(tokio::fs::read_to_string(path).await?);
    }
    let file = tokio::fs::File::open(path).await?;
    read_line_range(BufReader::new(file), offset, limit).await
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
