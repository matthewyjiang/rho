use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::*;

#[test]
fn schema_root_union_contains_only_object_branches() {
    let schema = EditFile.spec().input_schema;

    assert_eq!(schema["type"], "object");
    assert!(schema["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .all(|branch| branch["type"] == "object"));
}

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

#[tokio::test]
async fn replaces_unique_occurrence_with_legacy_arguments() {
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

#[tokio::test]
async fn applies_multiple_edits_across_multiple_files() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("first.txt"), "one old two old").unwrap();
    std::fs::write(ctx.cwd.join("second.txt"), "alpha").unwrap();

    let result = call(
        json!({"edits": [
            {
                "path": "first.txt",
                "old_string": "old",
                "new_string": "new",
                "expected_match_count": 2
            },
            {"path": "second.txt", "old_string": "alpha", "new_string": "beta"}
        ]}),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("first.txt")).unwrap(),
        "one new two new"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("second.txt")).unwrap(),
        "beta"
    );
    assert!(result
        .content
        .contains("edited 2 file(s); applied 2 edit(s), replaced 3 occurrence(s)"));
    assert!(result.content.contains("--- a/first.txt"));
    assert!(result.content.contains("--- a/second.txt"));
}

#[tokio::test]
async fn applies_multiple_edits_to_the_same_file_in_order() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha beta").unwrap();

    call(
        json!({"edits": [
            {"path": "sample.txt", "old_string": "alpha", "new_string": "gamma"},
            {"path": "sample.txt", "old_string": "gamma beta", "new_string": "done"}
        ]}),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "done"
    );
}

#[tokio::test]
async fn applies_same_file_edits_across_path_aliases() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha beta").unwrap();

    call(
        json!({"edits": [
            {"path": "sample.txt", "old_string": "alpha", "new_string": "gamma"},
            {"path": "./sample.txt", "old_string": "gamma beta", "new_string": "done"}
        ]}),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "done"
    );
}

#[tokio::test]
async fn applies_ordered_edits_across_hard_links() {
    let (_dir, ctx) = test_context();
    let original = ctx.cwd.join("original.txt");
    let alias = ctx.cwd.join("alias.txt");
    std::fs::write(&original, "alpha beta").unwrap();
    std::fs::hard_link(&original, &alias).unwrap();

    let result = call(
        json!({"edits": [
            {"path": "original.txt", "old_string": "alpha", "new_string": "gamma"},
            {"path": "alias.txt", "old_string": "gamma beta", "new_string": "done"}
        ]}),
        ctx,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(original).unwrap(), "done");
    assert_eq!(std::fs::read_to_string(alias).unwrap(), "done");
    assert!(result
        .content
        .contains("edited 1 file(s); applied 2 edit(s)"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rolls_back_earlier_file_when_a_later_write_fails() {
    fn file_changes(path: &std::path::Path, original: &str, updated: &str) -> FileChanges {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        FileChanges {
            path: path.to_path_buf(),
            file: tokio::fs::File::from_std(file.try_clone().unwrap()),
            identity: Handle::from_file(file).unwrap(),
            display_path: path.display().to_string(),
            original: original.into(),
            updated: updated.into(),
        }
    }

    let (root, _ctx) = test_context();
    let first_path = root.path().join("first.txt");
    std::fs::write(&first_path, "original").unwrap();
    let mut files = vec![
        file_changes(&first_path, "original", "updated"),
        file_changes(std::path::Path::new("/dev/full"), "", "cannot write"),
    ];

    let error = write_all_or_rollback(&mut files).await.unwrap_err();

    assert_eq!(std::fs::read_to_string(first_path).unwrap(), "original");
    assert!(error.to_string().contains("failed to write /dev/full"));
    assert!(error
        .to_string()
        .contains("rollback also failed for /dev/full"));
}

#[tokio::test]
async fn writes_nothing_when_a_later_edit_is_missing() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("first.txt"), "old").unwrap();
    std::fs::write(ctx.cwd.join("second.txt"), "present").unwrap();

    let error = call(
        json!({"edits": [
            {"path": "first.txt", "old_string": "old", "new_string": "new"},
            {"path": "second.txt", "old_string": "absent", "new_string": "new"}
        ]}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "edit 2 (second.txt) failed: missing match: found 0 occurrence(s), expected 1"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("first.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("second.txt")).unwrap(),
        "present"
    );
}

#[tokio::test]
async fn writes_nothing_when_a_later_edit_is_ambiguous() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("first.txt"), "old").unwrap();
    std::fs::write(ctx.cwd.join("second.txt"), "old old").unwrap();

    let error = call(
        json!({"edits": [
            {"path": "first.txt", "old_string": "old", "new_string": "new"},
            {"path": "second.txt", "old_string": "old", "new_string": "new"}
        ]}),
        ctx.clone(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "edit 2 (second.txt) failed: ambiguous match: found 2 occurrence(s), expected 1"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("first.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("second.txt")).unwrap(),
        "old old"
    );
}

#[tokio::test]
async fn reports_a_missing_expected_match() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "old").unwrap();

    let error = call(
        json!({"edits": [{
            "path": "sample.txt",
            "old_string": "old",
            "new_string": "new",
            "expected_match_count": 2
        }]}),
        ctx,
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "edit 1 (sample.txt) failed: missing match: found 1 occurrence(s), expected 2"
    );
}

#[tokio::test]
async fn expected_match_count_is_supported_for_legacy_arguments() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "old old").unwrap();

    call(
        json!({
            "path": "sample.txt",
            "old_string": "old",
            "new_string": "new",
            "expected_match_count": 2
        }),
        ctx.clone(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
        "new new"
    );
}

#[tokio::test]
async fn legacy_replace_all_remains_supported() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "old old").unwrap();

    let result = call(
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
        "new new"
    );
    assert!(result.content.contains("replaced 2 occurrence(s)"));
}

#[tokio::test]
async fn rejects_identical_old_and_new_string_with_edit_context() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

    let error = call(
        json!({"edits": [{
            "path": "sample.txt",
            "old_string": "alpha",
            "new_string": "alpha"
        }]}),
        ctx,
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "edit 1 (sample.txt) failed: old_string and new_string are identical; nothing to change"
    );
}

#[tokio::test]
async fn rejects_zero_expected_match_count() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

    let error = call(
        json!({"edits": [{
            "path": "sample.txt",
            "old_string": "alpha",
            "new_string": "beta",
            "expected_match_count": 0
        }]}),
        ctx,
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "edit 1 (sample.txt) failed: expected_match_count must be at least 1"
    );
}

#[tokio::test]
async fn edits_crlf_file_with_lf_tool_strings() {
    let (root, ctx) = test_context();
    let path = root.path().join("hello.txt");
    std::fs::write(&path, "one\r\ntwo\r\nthree\r\n").unwrap();

    call(
        json!({"edits": [{
            "path": "hello.txt",
            "old_string": "one\ntwo\n",
            "new_string": "1\n2\n"
        }]}),
        ctx,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "1\r\n2\r\nthree\r\n"
    );
}

#[tokio::test]
async fn edits_lf_file_with_crlf_tool_strings() {
    let (root, ctx) = test_context();
    let path = root.path().join("hello.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();

    call(
        json!({"edits": [{
            "path": "hello.txt",
            "old_string": "one\r\ntwo\r\n",
            "new_string": "1\r\n2\r\n"
        }]}),
        ctx,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "1\n2\nthree\n");
}

#[tokio::test]
async fn edits_bare_cr_file_with_lf_tool_strings() {
    let (root, ctx) = test_context();
    let path = root.path().join("hello.txt");
    std::fs::write(&path, "one\rtwo\rthree\r").unwrap();

    call(
        json!({"edits": [{
            "path": "hello.txt",
            "old_string": "one\ntwo\n",
            "new_string": "1\n2\n"
        }]}),
        ctx,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "1\r2\rthree\r");
}

#[tokio::test]
async fn multiple_replacements_preserve_mixed_line_endings_outside_matches() {
    let (root, ctx) = test_context();
    let path = root.path().join("hello.txt");
    std::fs::write(&path, "old\r\nkeep\nold\r\n").unwrap();

    call(
        json!({"edits": [{
            "path": "hello.txt",
            "old_string": "old\n",
            "new_string": "new\n",
            "expected_match_count": 2
        }]}),
        ctx,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "new\r\nkeep\nnew\r\n"
    );
}
