use pretty_assertions::assert_eq;
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

async fn call(args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult, ToolError> {
    EditFile.call(args, ctx, "call_1".into()).await
}

fn message(error: ToolError) -> String {
    let ToolError::Message(message) = error else {
        panic!("expected ToolError::Message, got {error:?}");
    };
    message
}

// Covers: unique single-file replace must rewrite the target and include a diff
// Owner: pure unit
#[tokio::test]
async fn replaces_unique_occurrence() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha beta gamma").unwrap();

    let result = call(
        json!({"path": "sample.txt", "old_string": "beta", "new_string": "delta"}),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert!(result.ok);
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "alpha delta gamma"
    );
    assert!(result.content.contains("--- a/sample.txt"));
    assert!(result.content.contains("+++ b/sample.txt"));
    assert!(result.content.contains("-alpha beta gamma"));
    assert!(result.content.contains("+alpha delta gamma"));
}

// Covers: replace_all must change every occurrence when the caller opts in
// Owner: pure unit
#[tokio::test]
async fn replace_all_updates_every_match() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "one old two old").unwrap();

    call(
        json!({
            "path": "sample.txt",
            "old_string": "old",
            "new_string": "new",
            "replace_all": true
        }),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "one new two new"
    );
}

// Covers: default mode must refuse ambiguous matches without mutating the file
// Owner: pure unit
#[tokio::test]
async fn rejects_ambiguous_match_without_mutation() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "old old").unwrap();

    let error = call(
        json!({"path": "sample.txt", "old_string": "old", "new_string": "new"}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "edit sample.txt failed: ambiguous match: found 2 occurrence(s), expected 1"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "old old"
    );
}

// Covers: missing match must fail closed
// Owner: pure unit
#[tokio::test]
async fn rejects_missing_match_without_mutation() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

    let error = call(
        json!({"path": "sample.txt", "old_string": "beta", "new_string": "gamma"}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "edit sample.txt failed: missing match: found 0 occurrence(s), expected 1"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "alpha"
    );
}

// Covers: identical strings must be rejected before IO-side mutation
// Owner: pure unit
#[tokio::test]
async fn rejects_identical_old_and_new_string() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

    let error = call(
        json!({"path": "sample.txt", "old_string": "alpha", "new_string": "alpha"}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "old_string and new_string are identical; nothing to change"
    );
}

// Covers: empty old_string must not create or rewrite files
// Owner: pure unit
#[tokio::test]
async fn rejects_empty_old_string() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

    let error = call(
        json!({"path": "sample.txt", "old_string": "", "new_string": "beta"}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(message(error), "old_string must not be empty");
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "alpha"
    );
}

// Covers: CRLF files must keep their line endings when tool args use LF
// Owner: pure unit
#[tokio::test]
async fn edits_crlf_file_with_lf_tool_strings() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha\r\nbeta\r\n").unwrap();

    call(
        json!({
            "path": "sample.txt",
            "old_string": "beta\n",
            "new_string": "gamma\n"
        }),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "alpha\r\ngamma\r\n"
    );
}

// Covers: missing target path must fail without creating a file
// Owner: pure unit
#[tokio::test]
async fn rejects_missing_file() {
    let (_dir, ctx) = test_context();

    let error = call(
        json!({"path": "missing.txt", "old_string": "a", "new_string": "b"}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    let msg = message(error);
    assert!(
        msg.starts_with("could not read missing.txt:"),
        "unexpected message: {msg}"
    );
    assert!(!ctx.cwd.join("missing.txt").exists());
}
