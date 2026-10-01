use std::path::Path;

use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

pub struct ListDir;
#[derive(Deserialize)]
struct Args {
    path: String,
}

impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".into(),
            description: "Lists the immediate entries of one directory, sorted by name, one per line. Directory names end with `/`. Not recursive, and it does not skip hidden or ignored entries; use `glob` to find files by pattern across a tree. Output beyond the tool-output limit is truncated.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string","description":"Directory to list, absolute or relative to the working directory."}},"required":["path"]}),
        }
    }

    fn call<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            let args: Args = serde_json::from_value(args)?;
            let path = resolve_path(&ctx.cwd, &args.path);
            let content = list_directory(&path).await?;
            Ok(ToolResult {
                id,
                ok: true,
                content: truncate(content, ctx.max_output_bytes),
            })
        })
    }
}

/// Script-facing listing: entries in text order, kept while their text fits
/// `max_output_bytes` (the same budget that truncates the text result).
pub(crate) fn structured_listing(content: &str, max_output_bytes: usize) -> serde_json::Value {
    let mut used = 0usize;
    let mut entries = Vec::new();
    let mut truncated = false;
    for line in content.lines() {
        used = used.saturating_add(line.len() + 1);
        if used > max_output_bytes {
            truncated = true;
            break;
        }
        let (name, kind) = match line.strip_suffix('/') {
            Some(name) => (name, "dir"),
            None => (line, "file"),
        };
        entries.push(json!({"name": name, "kind": kind}));
    }
    json!({"entries": entries, "truncated": truncated})
}

/// JSON Schema for [`structured_listing`].
pub(crate) fn list_dir_output_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "entries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "kind": {"type": "string", "enum": ["file", "dir"]}
                    },
                    "required": ["name", "kind"]
                }
            },
            "truncated": {"type": "boolean", "description": "Entries beyond the tool-output limit were dropped"}
        },
        "required": ["entries", "truncated"]
    })
}

pub(super) async fn list_directory(path: &Path) -> Result<String, ToolError> {
    let mut lines = Vec::new();
    let mut entries = tokio::fs::read_dir(path).await?;
    while let Some(entry) = entries.next_entry().await? {
        let ty = entry.file_type().await?;
        let suffix = if ty.is_dir() { "/" } else { "" };
        lines.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
    }
    lines.sort();
    Ok(lines.join("\n"))
}
