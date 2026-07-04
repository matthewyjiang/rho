use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

pub struct EditFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait::async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Edits an existing UTF-8 text file by exact string replacement.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"]}),
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
            "edit_file {}",
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
        if args.old_string.is_empty() {
            return Err(ToolError::Message("old_string must not be empty".into()));
        }
        if args.old_string == args.new_string {
            return Err(ToolError::Message(
                "old_string and new_string are identical; nothing to change".into(),
            ));
        }
        let path = resolve_path(&ctx.cwd, &args.path);
        let content = std::fs::read_to_string(&path)?;
        let count = content.matches(&args.old_string).count();
        if count == 0 {
            return Err(ToolError::Message("old_string not found in file".into()));
        }
        if !args.replace_all && count != 1 {
            return Err(ToolError::Message(format!(
                "old_string appeared {count} times, expected exactly once"
            )));
        }
        let new_content = if args.replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };
        std::fs::write(&path, new_content)?;
        Ok(ToolResult {
            id,
            ok: true,
            content: format!(
                "edited {}; replaced {} occurrence(s)",
                path.display(),
                count
            ),
        })
    }
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
    async fn replaces_unique_occurrence() {
        let (_dir, ctx) = test_context();
        fs::write(ctx.cwd.join("sample.txt"), "alpha beta gamma").unwrap();

        let result = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "beta", "new_string": "delta"}),
                ctx.clone(),
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
            "alpha delta gamma"
        );
    }

    #[tokio::test]
    async fn rejects_identical_old_and_new_string() {
        let (_dir, ctx) = test_context();
        fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

        let err = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "alpha", "new_string": "alpha"}),
                ctx,
                "call_1".into(),
            )
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "old_string and new_string are identical; nothing to change"
        );
    }

    #[tokio::test]
    async fn reports_missing_old_string() {
        let (_dir, ctx) = test_context();
        fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

        let err = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "missing", "new_string": "x"}),
                ctx,
                "call_1".into(),
            )
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "old_string not found in file");
    }
}
