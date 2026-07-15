use std::path::Path;

use crate::{
    tool::*,
    tools::diff::{unified_diff, UNREADABLE_FILE_DIFF_MESSAGE},
};
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

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let outcome = write_file_content(
            &path,
            &compact_display_path(&ctx.cwd, &args.path),
            &args.content,
            ctx.max_output_bytes,
        )
        .await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: outcome.content,
        })
    }
}

pub(super) struct WriteFileOutcome {
    pub content: String,
    pub display_path: String,
    pub diff: String,
}

pub(super) async fn write_file_content(
    path: &Path,
    display_path: &str,
    content: &str,
    max_output_bytes: usize,
) -> Result<WriteFileOutcome, ToolError> {
    let (old_content, existing_file_is_unreadable) = match tokio::fs::read_to_string(path).await {
        Ok(content) => (Some(content), false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, false),
        Err(_) => (None, true),
    };

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let diff = if existing_file_is_unreadable {
        UNREADABLE_FILE_DIFF_MESSAGE.into()
    } else {
        unified_diff(
            old_content.as_deref().unwrap_or(""),
            content,
            display_path,
            old_content.is_none(),
        )
    };
    tokio::fs::write(path, content).await?;

    let created = old_content.is_none() && !existing_file_is_unreadable;
    let action = if created { "created" } else { "wrote" };
    Ok(WriteFileOutcome {
        content: truncate(
            format!("{action} {}\n\n{diff}", path.display()),
            max_output_bytes,
        ),
        display_path: display_path.to_string(),
        diff,
    })
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
    async fn overwrites_unreadable_file_without_diff() {
        let (root, ctx) = test_context();
        let path = root.path().join("binary.bin");
        std::fs::write(&path, [0xFF]).unwrap();

        let result = WriteFile
            .call(
                json!({"path":"binary.bin","content":"replacement"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "replacement");
        assert!(result.content.contains("wrote "));
        assert!(result.content.contains(UNREADABLE_FILE_DIFF_MESSAGE));
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
