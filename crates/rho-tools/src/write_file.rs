use std::path::Path;

use crate::{
    diff::{unified_diff, UNREADABLE_FILE_DIFF_MESSAGE},
    tool::*,
};
use serde::Deserialize;
use serde_json::json;

pub struct WriteFile;

/// Shared result for single-path file mutations that return a unified diff.
pub(crate) struct FileMutationOutcome {
    pub content: String,
    pub display_path: String,
    pub diff: String,
}

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
            description: "Creates or fully rewrites a UTF-8 text file with complete contents. Prefer edit_file for one surgical string replacement, hashline_edit for multi-hunk line-anchored edits after read_file, and apply_patch for Codex-style multi-file patches that add or delete files.".into(),
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

pub(super) async fn write_file_content(
    path: &Path,
    display_path: &str,
    content: &str,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
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
    Ok(FileMutationOutcome {
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
        assert!(result.content.contains(UNREADABLE_FILE_DIFF_MESSAGE));
    }
}
