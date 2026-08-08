use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

fn test_context() -> (TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 12_000,
    };
    (dir, ctx)
}

fn message(error: ToolError) -> String {
    let ToolError::Message(message) = error else {
        panic!("expected ToolError::Message, got {error:?}");
    };
    message
}

// Covers: invalid replacement requests fail before any file mutation.
// Owner: edit_file argument validation
#[test]
fn rejects_invalid_replacement_arguments() {
    let cases = [
        ("", "new", "old_string must not be empty"),
        (
            "same",
            "same",
            "old_string and new_string are identical after newline normalization; nothing to change",
        ),
        (
            "same\n",
            "same\r\n",
            "old_string and new_string are identical after newline normalization; nothing to change",
        ),
    ];

    for (old_string, new_string, expected) in cases {
        let args = StrReplaceArgs {
            path: "sample.txt".into(),
            old_string: old_string.into(),
            new_string: new_string.into(),
            replace_all: false,
        };
        assert_eq!(message(args.validate().unwrap_err()), expected);
    }
}

// Covers: a unique replacement rewrites the target and returns chainable output.
// Owner: edit_file application
#[tokio::test]
async fn replaces_unique_occurrence_with_hashline_snapshot() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.txt");
    std::fs::write(&path, "alpha beta gamma").unwrap();

    let outcome = edit_file_content(
        &path,
        "sample.txt",
        "beta",
        "delta",
        /*replace_all*/ false,
        ctx.max_output_bytes,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha delta gamma");
    assert_eq!(outcome.display_paths, vec!["sample.txt"]);
    assert!(
        outcome.content.contains("[sample.txt#"),
        "{}",
        outcome.content
    );
    assert!(outcome.content.contains("1:alpha delta gamma"));
    assert!(!outcome.content.contains("@@"));
    assert!(outcome.diff.contains("-alpha beta gamma"));
    assert!(outcome.diff.contains("+alpha delta gamma"));
}

// Covers: ambiguous default matching fails closed instead of guessing.
// Owner: edit_file application
#[tokio::test]
async fn rejects_ambiguous_match_without_mutation() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.txt");
    std::fs::write(&path, "old old").unwrap();

    let error = edit_file_content(
        &path,
        "sample.txt",
        "old",
        "new",
        /*replace_all*/ false,
        ctx.max_output_bytes,
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "edit sample.txt failed: ambiguous match: found 2 occurrence(s), expected 1"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "old old");
}

// Covers: newline-normalized matching preserves the target file's CRLF style.
// Owner: edit_file application
#[tokio::test]
async fn preserves_crlf_when_arguments_use_lf() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.txt");
    std::fs::write(&path, "alpha\r\nbeta\r\n").unwrap();

    edit_file_content(
        &path,
        "sample.txt",
        "beta\n",
        "gamma\n",
        /*replace_all*/ false,
        ctx.max_output_bytes,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha\r\ngamma\r\n");
}

// Covers: edit_file_content honors replace_all and reports the applied count.
// Owner: edit_file public content path
#[tokio::test]
async fn replaces_all_occurrences_through_edit_file_content() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("sample.txt");
    std::fs::write(&path, "old middle old\n").unwrap();

    let outcome = edit_file_content(
        &path,
        "sample.txt",
        "old",
        "new",
        /*replace_all*/ true,
        ctx.max_output_bytes,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "new middle new\n");
    assert!(outcome
        .content
        .starts_with("edited sample.txt; replaced 2 occurrence(s)"));
}

#[cfg(unix)]
// Covers: edit_file rejects a symlink leaf instead of rewriting its target.
// Owner: edit_file locked path validation
#[tokio::test]
async fn rejects_symlink_leaf_without_mutating_target() {
    use std::os::unix::fs::symlink;

    let (_dir, ctx) = test_context();
    let target = ctx.cwd.join("target.txt");
    let alias = ctx.cwd.join("alias.txt");
    std::fs::write(&target, "old\n").unwrap();
    symlink(&target, &alias).unwrap();

    let error = edit_file_content(
        &alias,
        "alias.txt",
        "old",
        "new",
        /*replace_all*/ false,
        ctx.max_output_bytes,
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "edit alias.txt failed: path changed after validation"
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "old\n");
    assert!(alias.is_symlink());
}
