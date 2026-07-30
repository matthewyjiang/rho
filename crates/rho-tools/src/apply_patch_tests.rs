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

    let ToolError::Message(message) = error else {
        panic!("expected ToolError::Message, got {error:?}");
    };
    assert_eq!(
        message,
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
    let ToolError::Message(message) = error else {
        panic!("expected ToolError::Message, got {error:?}");
    };
    assert!(
        message.starts_with("Failed to read file to update missing.txt:"),
        "unexpected message: {message}"
    );
}

#[tokio::test]
async fn rejects_absolute_and_parent_paths() {
    let (_dir, ctx) = test_context();
    let absolute = call(
        "*** Begin Patch\n*** Add File: /tmp/evil.txt\n+nope\n*** End Patch",
        ctx.clone(),
    )
    .await
    .unwrap_err();
    let ToolError::Message(message) = absolute else {
        panic!("expected ToolError::Message, got {absolute:?}");
    };
    assert_eq!(message, "patch path must be relative: /tmp/evil.txt");

    let parent = call(
        "*** Begin Patch\n*** Add File: ../escape.txt\n+nope\n*** End Patch",
        ctx,
    )
    .await
    .unwrap_err();
    let ToolError::Message(message) = parent else {
        panic!("expected ToolError::Message, got {parent:?}");
    };
    assert_eq!(message, "patch path must not contain '..': ../escape.txt");
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

#[test]
fn patch_paths_extracts_add_update_move_and_delete() {
    let paths = patch_paths(
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
