use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::tool::ToolContext;

fn test_context() -> (TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 12_000,
    };
    (dir, ctx)
}

// Covers: end-to-end edit must rewrite the target and return a chainable preview
// Owner: edit tool
#[tokio::test]
async fn edits_file_from_read_tag() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.rs");
    let original = "fn main() {\n    println!(\"hi\");\n}\n";
    std::fs::write(&path, original).unwrap();
    let tag = compute_file_hash(original);
    let input = format!("[sample.rs#{tag}]\nPUT 2.=2:\n+    println!(\"hello\");\n");

    let result = Edit
        .call(
            serde_json::json!({ "input": input }),
            ctx,
            "call_edit".into(),
        )
        .await
        .unwrap();

    let updated = std::fs::read_to_string(path).unwrap();
    assert_eq!(updated, "fn main() {\n    println!(\"hello\");\n}\n");
    let new_tag = compute_file_hash(&updated);
    assert!(
        result.content.contains(&format!("[sample.rs#{new_tag}]")),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("2:    println!(\"hello\");"),
        "{}",
        result.content
    );
    // Chain contract: preview only - no unified diff in model content.
    assert!(
        !result.content.contains("@@"),
        "model content should not embed unified diff: {}",
        result.content
    );
}

// Covers: a second edit must succeed using only the first edit's returned preview
// Owner: edit tool
#[tokio::test]
async fn chains_second_edit_from_post_edit_preview_without_reread() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("chain.txt");
    let original = "alpha\nbeta\ngamma\n";
    std::fs::write(&path, original).unwrap();
    let tag = compute_file_hash(original);
    let first = Edit
        .call(
            serde_json::json!({
                "input": format!("[chain.txt#{tag}]\nPUT 2.=2:\n+BETA\n")
            }),
            ToolContext {
                cwd: ctx.cwd.clone(),
                max_output_bytes: ctx.max_output_bytes,
            },
            "call_first".into(),
        )
        .await
        .unwrap();

    let mid = std::fs::read_to_string(&path).unwrap();
    assert_eq!(mid, "alpha\nBETA\ngamma\n");
    let new_tag = compute_file_hash(&mid);
    assert!(
        first.content.contains(&format!("[chain.txt#{new_tag}]")),
        "{}",
        first.content
    );

    Edit.call(
        serde_json::json!({
            "input": format!("[chain.txt#{new_tag}]\nPUT 3.=3:\n+GAMMA\n")
        }),
        ctx,
        "call_second".into(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "alpha\nBETA\nGAMMA\n"
    );
}

// Covers: stale tags must leave the file unchanged
// Owner: edit tool
#[tokio::test]
async fn stale_tag_leaves_file_untouched() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.rs");
    let original = "alpha\nbeta\n";
    std::fs::write(&path, original).unwrap();

    let error = Edit
        .call(
            serde_json::json!({
                "input": "[sample.rs#DEADBEEF]\nPUT 1.=1:\n+nope\n"
            }),
            ctx,
            "call_stale".into(),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("tag mismatch"), "{error}");
    assert!(error.to_string().contains("Live snapshot"), "{error}");
    let live_tag = compute_file_hash(original);
    assert!(
        error
            .to_string()
            .contains(&format!("[sample.rs#{live_tag}]")),
        "{error}"
    );
    assert!(error.to_string().contains("1:alpha"), "{error}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

// Covers: multi-file documents must edit every section under one call
// Owner: edit tool
#[tokio::test]
async fn edits_multiple_files_in_one_document() {
    let (_dir, ctx) = test_context();
    let a_path = ctx.cwd.join("a.txt");
    let b_path = ctx.cwd.join("b.txt");
    let a = "one\n";
    let b = "two\n";
    std::fs::write(&a_path, a).unwrap();
    std::fs::write(&b_path, b).unwrap();
    let input = format!(
        "[a.txt#{}]\nPUT 1.=1:\n+ONE\n\n[b.txt#{}]\nPUT 1.=1:\n+TWO\n",
        compute_file_hash(a),
        compute_file_hash(b)
    );

    Edit.call(
        serde_json::json!({ "input": input }),
        ctx,
        "call_multi".into(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(a_path).unwrap(), "ONE\n");
    assert_eq!(std::fs::read_to_string(b_path).unwrap(), "TWO\n");
}
