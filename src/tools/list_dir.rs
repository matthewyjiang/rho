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

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_or_command()
    }

    fn display_content(&self, args: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
        args.get("path")
            .and_then(|path| path.as_str())
            .map(|path| compact_display_path(&ctx.cwd, path))
    }

    fn display_lines(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
        result: &ToolResult,
    ) -> Vec<String> {
        vec![format!(
            "list_dir {}",
            self.display_content(args, ctx)
                .unwrap_or_else(|| result.content.clone())
        )]
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let mut lines = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let suffix = if ty.is_dir() { "/" } else { "" };
            lines.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
        }
        lines.sort();
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(lines.join("\n"), ctx.max_output_bytes),
        })
    }
}
