use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

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

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_or_command()
    }

    fn display_content(&self, args: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
        args.get("path")
            .and_then(|path| path.as_str())
            .map(|path| read_file_display_content(&ctx.cwd, path, args))
    }

    fn display_lines(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
        result: &ToolResult,
    ) -> Vec<String> {
        vec![format!(
            "read_file {}",
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
        let content = std::fs::read_to_string(resolve_path(&ctx.cwd, &args.path))?;
        let content = select_line_range(&content, args.offset, args.limit)?;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(content, ctx.max_output_bytes),
        })
    }
}

fn read_file_display_content(
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

fn select_line_range(
    content: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    if offset == Some(0) {
        return Err(ToolError::Message("offset must be greater than 0".into()));
    }
    if limit == Some(0) {
        return Err(ToolError::Message("limit must be greater than 0".into()));
    }
    if offset.is_none() && limit.is_none() {
        return Ok(content.to_string());
    }

    let start = offset.unwrap_or(1) - 1;
    let total_lines = content.split_inclusive('\n').count();
    if start > 0 && start >= total_lines {
        return Err(ToolError::Message(format!(
            "offset {} is past the end of the file ({total_lines} line(s))",
            start + 1
        )));
    }
    let lines = content.split_inclusive('\n').skip(start);
    let selected = match limit {
        Some(limit) => lines.take(limit).collect(),
        None => lines.collect(),
    };
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use std::fs;

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
}
