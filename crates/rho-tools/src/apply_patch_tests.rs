use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::tool::{Tool, ToolContext, ToolError, ToolResult};

fn test_context() -> (TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 12000,
    };
    (dir, ctx)
}

async fn call(input: &str, ctx: ToolContext) -> Result<ToolResult, ToolError> {
    ApplyPatch
        .call(json!({"input": input}), ctx, "call_1".into())
        .await
}

fn message(error: ToolError) -> String {
    let ToolError::Message(message) = error else {
        panic!("expected ToolError::Message, got {error:?}");
    };
    message
}

#[test]
fn schema_requires_input_string() {
    let schema = ApplyPatch.spec().input_schema;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["input"]));
    assert_eq!(schema["properties"]["input"]["type"], "string");
}

#[tokio::test]
async fn applies_add_update_and_delete() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("modify.txt"), "line1\nline2\n").unwrap();
    std::fs::write(ctx.cwd.join("delete.txt"), "obsolete\n").unwrap();

    let result = call(
        "*** Begin Patch\n*** Add File: nested/new.txt\n+created\n*** Delete File: delete.txt\n*** Update File: modify.txt\n@@\n-line2\n+changed\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap();

    assert!(result.ok);
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("nested/new.txt")).unwrap(),
        "created\n"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("modify.txt")).unwrap(),
        "line1\nchanged\n"
    );
    assert!(!ctx.cwd.join("delete.txt").exists());
    assert!(result.content.contains("A nested/new.txt"));
    assert!(result.content.contains("M modify.txt"));
    assert!(result.content.contains("D delete.txt"));
    assert!(
        result.content.contains("--- a/modify.txt") || result.content.contains("+++ b/modify.txt")
    );
}

#[tokio::test]
async fn applies_multiple_chunks_in_one_file() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("multi.txt"), "line1\nline2\nline3\nline4\n").unwrap();

    call(
        "*** Begin Patch\n*** Update File: multi.txt\n@@\n-line2\n+changed2\n@@\n-line4\n+changed4\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("multi.txt")).unwrap(),
        "line1\nchanged2\nline3\nchanged4\n"
    );
}

#[tokio::test]
async fn moves_file_to_new_directory() {
    let (_dir, ctx) = test_context();
    std::fs::create_dir_all(ctx.cwd.join("old")).unwrap();
    std::fs::write(ctx.cwd.join("old/name.txt"), "old content\n").unwrap();

    call(
        "*** Begin Patch\n*** Update File: old/name.txt\n*** Move to: renamed/dir/name.txt\n@@\n-old content\n+new content\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap();

    assert!(!ctx.cwd.join("old/name.txt").exists());
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("renamed/dir/name.txt")).unwrap(),
        "new content\n"
    );
}

#[tokio::test]
async fn rejects_missing_context_without_mutating() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("modify.txt"), "line1\nline2\n").unwrap();

    let error = call(
        "*** Begin Patch\n*** Update File: modify.txt\n@@\n-missing\n+changed\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "Failed to find expected lines in modify.txt:\nmissing"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("modify.txt")).unwrap(),
        "line1\nline2\n"
    );
}

#[test]
fn rejects_empty_update_hunk() {
    let error = parse_patch("*** Begin Patch\n*** Update File: foo.txt\n*** End Patch")
        .expect_err("empty update must fail parse");
    assert_eq!(
        error,
        ParseError::InvalidHunk {
            message: "Update file hunk for path 'foo.txt' is empty".into(),
            line_number: 2,
        }
    );
}

#[tokio::test]
async fn rejects_missing_update_target() {
    let (_dir, ctx) = test_context();
    let error = call(
        "*** Begin Patch\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch",
        ctx,
    )
    .await
    .unwrap_err();
    let msg = message(error);
    assert!(
        msg.starts_with("Failed to read file to update missing.txt:"),
        "unexpected message: {msg}"
    );
}

#[tokio::test]
async fn rejects_absolute_and_parent_paths() {
    let (_dir, ctx) = test_context();
    let absolute_path = if cfg!(windows) {
        r"C:\tmp\evil.txt"
    } else {
        "/tmp/evil.txt"
    };
    let absolute = call(
        &format!("*** Begin Patch\n*** Add File: {absolute_path}\n+nope\n*** End Patch"),
        ctx.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(absolute),
        format!("patch path must be relative: {absolute_path}")
    );

    // Also reject Unix-root form on every platform (Windows treats `/tmp/...`
    // as relative under `Path::is_absolute`, but RootDir must still be blocked).
    let unix_root = call(
        "*** Begin Patch\n*** Add File: /tmp/evil.txt\n+nope\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(unix_root),
        "patch path must be relative: /tmp/evil.txt"
    );

    let parent = call(
        "*** Begin Patch\n*** Add File: ../escape.txt\n+nope\n*** End Patch",
        ctx,
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(parent),
        "patch path must not contain '..': ../escape.txt"
    );
}

#[tokio::test]
async fn rejects_move_to_existing_destination() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("src.txt"), "source\n").unwrap();
    std::fs::write(ctx.cwd.join("dst.txt"), "existing\n").unwrap();

    let error = call(
        "*** Begin Patch\n*** Update File: src.txt\n*** Move to: dst.txt\n@@\n-source\n+moved\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "Refusing to move to 'dst.txt': destination already exists"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("src.txt")).unwrap(),
        "source\n"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("dst.txt")).unwrap(),
        "existing\n"
    );
}

#[tokio::test]
async fn pure_addition_inserts_at_context_cursor() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha\nbeta\ngamma\n").unwrap();

    call(
        "*** Begin Patch\n*** Update File: sample.txt\n@@ alpha\n+inserted\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "alpha\ninserted\nbeta\ngamma\n"
    );
}

#[tokio::test]
async fn pure_addition_keeps_later_context_on_original_coordinates() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha\nbeta\ngamma\n").unwrap();

    // Inserting multiple lines must not advance the original-line cursor past
    // later chunks; otherwise gamma would be skipped after a 3-line insert.
    call(
        "*** Begin Patch\n*** Update File: sample.txt\n@@ alpha\n+i1\n+i2\n+i3\n@@\n-gamma\n+GAMMA\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "alpha\ni1\ni2\ni3\nbeta\nGAMMA\n"
    );
}

#[test]
fn patch_paths_lenient_extracts_add_update_move_and_delete() {
    let paths = patch_paths_lenient(
        "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** Delete File: gone.txt\n*** End Patch",
    );
    assert_eq!(
        paths,
        vec![
            "a.txt".to_string(),
            "old.txt".to_string(),
            "new.txt".to_string(),
            "gone.txt".to_string()
        ]
    );
}

#[test]
fn patch_paths_lenient_returns_empty_for_invalid_input() {
    assert!(patch_paths_lenient("not a patch").is_empty());
}

#[tokio::test]
async fn rejects_delete_and_move_of_same_source() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("a.txt"), "body\n").unwrap();

    let error = call(
        "*** Begin Patch\n*** Delete File: a.txt\n*** Update File: a.txt\n*** Move to: b.txt\n@@\n-body\n+body\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert!(message(error).contains("deletes"), "expected path conflict");
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("a.txt")).unwrap(),
        "body\n"
    );
    assert!(!ctx.cwd.join("b.txt").exists());
}

#[tokio::test]
async fn fails_closed_when_file_changes_after_plan() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(ctx.cwd.join("b.txt"), "beta\n").unwrap();
    let cwd = ctx.cwd.clone();
    let hunks = parse_patch(
        "*** Begin Patch\n*** Update File: a.txt\n@@\n-alpha\n+ALPHA\n*** Update File: b.txt\n@@\n-beta\n+BETA\n*** End Patch",
    )
    .unwrap();

    // While resolving b.txt, mutate a.txt. This relies on plan_hunk resolving
    // and snapshotting a.txt before b.txt so the race is observable at commit.
    let error = apply_hunks(
        hunks,
        {
            let cwd = cwd.clone();
            move |requested| {
                if requested == "b.txt" {
                    std::fs::write(cwd.join("a.txt"), "tampered\n").unwrap();
                }
                Ok(cwd.join(requested))
            }
        },
        |requested| requested.to_string(),
        12_000,
    )
    .await
    .unwrap_err();

    let msg = message(error);
    assert!(
        msg.contains("changed while the patch was being validated"),
        "unexpected message: {msg}"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("a.txt")).unwrap(),
        "tampered\n"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("b.txt")).unwrap(),
        "beta\n"
    );
}

#[tokio::test]
async fn preserves_missing_trailing_newline() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("nonewline.txt"), "alpha\nbeta").unwrap();

    call(
        "*** Begin Patch\n*** Update File: nonewline.txt\n@@\n-beta\n+beta2\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("nonewline.txt")).unwrap(),
        "alpha\nbeta2"
    );
}

#[test]
fn rejects_environment_id_and_heredoc_wrappers() {
    let env = parse_patch(
        "*** Begin Patch\n*** Environment ID: abc\n*** Add File: a.txt\n+hi\n*** End Patch",
    )
    .unwrap_err();
    assert!(matches!(env, ParseError::InvalidHunk { .. }));

    let heredoc =
        parse_patch("<<EOF\n*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch\nEOF")
            .unwrap_err();
    assert!(matches!(heredoc, ParseError::InvalidPatch(_)));
}
