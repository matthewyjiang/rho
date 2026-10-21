use std::{path::Path, sync::Arc};

use crate::{
    diff::{unified_diff, UNREADABLE_FILE_DIFF_MESSAGE},
    file_mutation::FileMutationOutcome,
    host_files::{HostTextFiles, TextRoute},
    tool::*,
};
use serde::Deserialize;
use serde_json::json;

pub struct WriteFile;

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description: "Creates or fully rewrites a UTF-8 text file with complete contents. Prefer the selected edit tool for targeted changes. Successful writes return a bounded numbered snapshot for follow-up work without an extra `read_file`. When the selected edit tool is hashline, that snapshot is `[path#TAG]` plus numbered lines. Re-read only for lines outside that snapshot. Unified diff is tool metadata for UI cards.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
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
            let outcome = write_file_content(
                &path,
                &compact_display_path(&ctx.cwd, &args.path),
                &args.content,
                ctx.max_output_bytes,
                crate::FileViewStyle::Hashline,
                /*host_files*/ None,
            )
            .await?;
            Ok(ToolResult {
                id,
                ok: true,
                content: outcome.content,
            })
        })
    }
}

/// Writes a whole file. A path the host serves takes its old text from the
/// host and its new text goes to the host, unless the file on disk is not
/// UTF-8 text, which no editor buffer holds. The result shows the text the
/// file holds afterwards, which a host may have formatted.
pub(super) async fn write_file_content(
    path: &Path,
    display_path: &str,
    content: &str,
    max_output_bytes: usize,
    style: crate::FileViewStyle,
    host_files: Option<&Arc<dyn HostTextFiles>>,
) -> Result<FileMutationOutcome, ToolError> {
    let route = TextRoute::for_path(host_files, path);
    let (route, old_content, existing_file_is_unreadable) =
        match (tokio::fs::read_to_string(path).await, route.is_host()) {
            // No editor buffer holds a file that is not UTF-8 text.
            (Err(error), _) if error.kind() == std::io::ErrorKind::InvalidData => {
                (TextRoute::Disk, None, true)
            }
            // A host can hold a file it has not saved yet.
            (Err(error), true) if error.kind() == std::io::ErrorKind::NotFound => {
                let old_content = route.read_if_held(path, display_path).await?;
                (route, old_content, false)
            }
            (_, true) => {
                let old_content = route.read(path, display_path).await?;
                (route, Some(old_content), false)
            }
            (Err(error), false) if error.kind() == std::io::ErrorKind::NotFound => {
                (route, None, false)
            }
            (Err(_), false) => (route, None, true),
            (Ok(disk), false) => (route, Some(disk), false),
        };

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let written = route.write(path, display_path, content).await?;
    let content = written.as_str();
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

    let created = old_content.is_none() && !existing_file_is_unreadable;
    // An unreadable existing file is not a creation: do not invent its old text.
    let file_changes = if existing_file_is_unreadable {
        Vec::new()
    } else {
        vec![match old_content {
            Some(original) => rho_sdk::tool::ToolFileChange::modified(path, original, content),
            None => rho_sdk::tool::ToolFileChange::created(path, content),
        }]
    };
    let action = if created { "created" } else { "wrote" };
    // Model-facing chain contract: action line + bounded hashline snapshot.
    // Unified diff stays on metadata for UI cards.
    let snapshot = style.format_chain_snapshot(display_path, content, &[]);
    let content = if existing_file_is_unreadable {
        format!("{action} {display_path}\n\n{UNREADABLE_FILE_DIFF_MESSAGE}\n\n{snapshot}")
    } else {
        format!("{action} {display_path}\n\n{snapshot}")
    };
    Ok(FileMutationOutcome {
        content: truncate(content, max_output_bytes),
        display_paths: vec![display_path.to_string()],
        diff,
        file_changes,
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

    // Covers: a host that formats on save: the result's snapshot and file
    // change show the text the host holds, so the model's next edit tags it.
    // Owner: write, through host_files
    #[tokio::test]
    async fn shows_the_text_a_formatting_host_saved() {
        use crate::host_files::test_support::MemoryHost;

        let (root, _) = test_context();
        let path = root.path().join("a.txt");
        let mut host = MemoryHost::new(root.path(), &[]);
        host.format = str::to_uppercase;
        let host: Arc<dyn HostTextFiles> = Arc::new(host);

        let outcome = write_file_content(
            &path,
            "a.txt",
            "draft\n",
            12_000,
            crate::FileViewStyle::Hashline,
            Some(&host),
        )
        .await
        .unwrap();

        assert_eq!(
            (outcome.content, outcome.file_changes),
            (
                format!(
                    "created a.txt\n\n{}",
                    crate::FileViewStyle::Hashline.format_chain_snapshot("a.txt", "DRAFT\n", &[])
                ),
                vec![rho_sdk::tool::ToolFileChange::created(&path, "DRAFT\n")],
            )
        );
    }

    // Covers: a file the host holds but has not saved to disk is replaced,
    // not created again: its host text is the diff's old side.
    // Owner: write, through host_files
    #[tokio::test]
    async fn rewrites_a_file_only_the_host_holds() {
        use crate::host_files::test_support::MemoryHost;

        let (root, _) = test_context();
        let path = root.path().join("a.txt");
        let host: Arc<dyn HostTextFiles> =
            Arc::new(MemoryHost::new(root.path(), &[(&path, "unsaved\n")]));

        let outcome = write_file_content(
            &path,
            "a.txt",
            "next\n",
            12_000,
            crate::FileViewStyle::Hashline,
            Some(&host),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome.file_changes,
            vec![rho_sdk::tool::ToolFileChange::modified(
                &path,
                "unsaved\n",
                "next\n"
            )]
        );
    }

    // Covers: unreadable existing files still rewrite and keep the special diff notice
    // Owner: write
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
}
