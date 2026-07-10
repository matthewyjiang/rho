use crate::{tool::*, tools::diff::unified_diff};
use serde::Deserialize;
use serde_json::json;

pub struct WriteFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".into(),
            description: "Writes a UTF-8 text file, creating or overwriting it.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        }
    }

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_diff()
    }

    fn display_content(&self, args: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
        args.get("path")
            .and_then(|path| path.as_str())
            .map(|path| compact_display_path(&ctx.cwd, path))
    }

    fn display_start_lines(&self, args: &serde_json::Value, ctx: &ToolContext) -> Vec<String> {
        vec![format!(
            "write_file {}",
            self.display_content(args, ctx).unwrap_or_default()
        )]
    }

    fn display_lines(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
        result: &ToolResult,
    ) -> Vec<String> {
        let mut lines = vec![format!(
            "write_file {}",
            self.display_content(args, ctx)
                .unwrap_or_else(|| result.content.clone())
        )];
        if result.ok {
            if let Some(diff) = super::diff::compact_diff_for_display(&result.content) {
                lines.push(diff);
            }
        }
        lines
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let old_content = match std::fs::read_to_string(&path) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let diff = unified_diff(
            old_content.as_deref().unwrap_or(""),
            &args.content,
            &compact_display_path(&ctx.cwd, &args.path),
            old_content.is_none(),
        );
        std::fs::write(&path, args.content)?;

        let action = if old_content.is_none() {
            "created"
        } else {
            "wrote"
        };
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(
                format!("{action} {}\n\n{diff}", path.display()),
                ctx.max_output_bytes,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
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
    async fn writes_file_and_creates_parent_dirs() {
        let (root, ctx) = test_context();
        let result = WriteFile
            .call(
                json!({"path":"nested/hello.txt","content":"hello"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(root.path().join("nested/hello.txt")).unwrap(),
            "hello"
        );
        assert!(result.content.contains("created "));
        assert!(result.content.contains("--- /dev/null"));
        assert!(result.content.contains("+++ b/nested/hello.txt"));
        assert!(result.content.contains("+hello"));
    }

    #[tokio::test]
    async fn reports_overwritten_file() {
        let (root, ctx) = test_context();
        std::fs::write(root.path().join("hello.txt"), "hello\nold\n").unwrap();

        let result = WriteFile
            .call(
                json!({"path":"hello.txt","content":"hello\nnew\n"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.content.contains("wrote "));
        assert!(result.content.contains("--- a/hello.txt"));
        assert!(result.content.contains("+++ b/hello.txt"));
        assert!(result.content.contains("-old"));
        assert!(result.content.contains("+new"));
    }
}
