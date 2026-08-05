use std::path::Path;

use crate::{
    diff::{unified_diff, UNREADABLE_FILE_DIFF_MESSAGE},
    tool::*,
};
use serde::Deserialize;
use serde_json::json;

pub struct WriteFile;

/// Shared result for file mutations that return a unified diff.
pub(crate) struct FileMutationOutcome {
    pub content: String,
    /// Display paths touched by the mutation, in document order.
    pub display_paths: Vec<String>,
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
            description: "Creates or fully rewrites a UTF-8 text file with complete contents. Prefer `edit` for multi-hunk line-anchored edits when you already have a fresh `[path#TAG]`. Successful writes return a bounded hashline chain snapshot (`[path#TAG]` plus numbered lines) so a follow-up `edit` can start without an extra `read_file`. Re-read only for lines outside that snapshot. Unified diff is tool metadata for UI cards.".into(),
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
    // Model-facing chain contract: action line + bounded hashline snapshot.
    // Unified diff stays on metadata for UI cards.
    let snapshot = crate::hashline::format_chain_snapshot(display_path, content, &[]);
    let content = if existing_file_is_unreadable {
        format!("{action} {display_path}\n\n{UNREADABLE_FILE_DIFF_MESSAGE}\n\n{snapshot}")
    } else {
        format!("{action} {display_path}\n\n{snapshot}")
    };
    Ok(FileMutationOutcome {
        content: truncate(content, max_output_bytes),
        display_paths: vec![display_path.to_string()],
        diff,
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::hashline::compute_file_hash;

    fn test_context() -> (TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            max_output_bytes: 12000,
        };
        (dir, ctx)
    }

    // Covers: write creates nested paths and returns a chainable hashline snapshot
    // Owner: write_file
    #[tokio::test]
    async fn writes_file_and_creates_parent_dirs() {
        let (root, ctx) = test_context();
        let result = WriteFile
            .call(
                json!({"path":"nested/hello.txt","content":"hello\n"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(root.path().join("nested/hello.txt")).unwrap(),
            "hello\n"
        );
        let tag = compute_file_hash("hello\n");
        assert!(
            result
                .content
                .contains(&format!("[nested/hello.txt#{tag}]")),
            "{}",
            result.content
        );
        assert!(result.content.contains("1:hello"), "{}", result.content);
        assert!(
            !result.content.contains("@@"),
            "model content should not embed unified diff: {}",
            result.content
        );
    }

    // Covers: unreadable existing files still rewrite and keep the special diff notice
    // Owner: write_file
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
        assert!(
            result.content.contains("[binary.bin#"),
            "{}",
            result.content
        );
    }

    // Covers: large writes return a bounded head/tail snapshot, not the whole file
    // Owner: write_file
    #[tokio::test]
    async fn large_write_returns_bounded_chain_snapshot() {
        let (_root, ctx) = test_context();
        let mut body = String::new();
        for i in 1..=80 {
            body.push_str(&format!("line-{i}\n"));
        }
        let result = WriteFile
            .call(
                json!({"path":"big.txt","content": body}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.content.contains("1:line-1"), "{}", result.content);
        assert!(result.content.contains("80:line-80"), "{}", result.content);
        assert!(
            result.content.contains("showing"),
            "expected truncation notice: {}",
            result.content
        );
        // Middle of a large file should not all be dumped into model content.
        assert!(
            !result.content.contains("40:line-40"),
            "middle lines should be elided: {}",
            result.content
        );
    }
}
