use std::path::Path;

use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

pub struct ListDir;
#[derive(Deserialize)]
struct Args {
    path: String,
}

#[async_trait::async_trait]
impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".into(),
            description: "Lists a directory.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
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
        let content = list_directory(&path).await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(content, ctx.max_output_bytes),
        })
    }
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
