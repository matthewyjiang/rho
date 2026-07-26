use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::{glob_workspace, GlobRequest};
use crate::tool::{compact_display_path, resolve_path, ToolError};

/// Runs glob the way the SDK adapter does: parse, resolve the root against the
/// workspace, then walk.
fn call_glob(dir: &TempDir, args: serde_json::Value) -> Result<String, ToolError> {
    let request = GlobRequest::from_arguments(args)?;
    let root = resolve_path(dir.path(), &request.path);
    let display = compact_display_path(dir.path(), &request.path);
    glob_workspace(&root, &display, &request, &|| false)
}

fn write(dir: &TempDir, relative: &str, content: &str) {
    let path = dir.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn nested_pattern_returns_paths_in_directory_order() {
    let dir = TempDir::new().unwrap();
    write(&dir, "src/b.rs", "");
    write(&dir, "src/a.rs", "");
    write(&dir, "src/nested/c.rs", "");
    write(&dir, "src/a.txt", "");
    std::fs::create_dir_all(dir.path().join("src/empty")).unwrap();

    let content = call_glob(&dir, json!({"pattern": "**/*.rs"})).unwrap();
    assert_eq!(
        content,
        "\
src/a.rs
src/b.rs
src/nested/c.rs

3 files"
    );
}

#[test]
fn path_scopes_search_and_paths_are_relative_to_scope() {
    let dir = TempDir::new().unwrap();
    write(&dir, "crates/rho/src/lib.rs", "");
    write(&dir, "crates/other/src/lib.rs", "");

    let content = call_glob(&dir, json!({"pattern": "**/*.rs", "path": "crates/rho"})).unwrap();
    assert_eq!(
        content,
        "\
src/lib.rs

1 files"
    );
}

#[test]
fn directories_are_absent_from_results() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    write(&dir, "src/main.rs", "");

    let content = call_glob(&dir, json!({"pattern": "src/**"})).unwrap();
    assert!(content.contains("src/main.rs\n"), "{content}");
    assert!(!content.lines().any(|line| line == "src"), "{content}");
}

#[test]
fn gitignored_and_hidden_excluded_by_default() {
    let dir = TempDir::new().unwrap();
    write(&dir, ".gitignore", "ignored.rs\n");
    write(&dir, "ignored.rs", "");
    write(&dir, ".hidden.rs", "");
    write(&dir, "kept.rs", "");

    let content = call_glob(&dir, json!({"pattern": "*.rs"})).unwrap();
    assert_eq!(
        content,
        "\
kept.rs

1 files"
    );

    let hidden = call_glob(&dir, json!({"pattern": "*.rs", "include_hidden": true})).unwrap();
    assert!(hidden.contains(".hidden.rs\n"), "{hidden}");
    assert!(!hidden.contains("ignored.rs"), "{hidden}");
}

#[test]
fn max_results_takes_the_first_paths_in_walk_order() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.rs", "");
    write(&dir, "b.rs", "");
    write(&dir, "c.rs", "");

    let content = call_glob(&dir, json!({"pattern": "*.rs", "max_results": 2})).unwrap();
    assert_eq!(
        content,
        "\
a.rs
b.rs

2 files (result limit reached; narrow the pattern or path)"
    );
}

#[test]
fn no_matches_message() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "");
    let content = call_glob(&dir, json!({"pattern": "*.rs", "path": "."})).unwrap();
    assert_eq!(content, "no files matching '*.rs' under .");
}

#[test]
fn cancellation_is_reported_rather_than_read_as_no_matches() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.rs", "");
    let request = GlobRequest::from_arguments(json!({"pattern": "*.rs"})).unwrap();
    let out = glob_workspace(dir.path(), ".", &request, &|| true).unwrap();
    assert_eq!(out, "no files matching '*.rs' under . (cancelled)");
}

#[test]
fn empty_results_report_each_incomplete_stop_reason() {
    use crate::{
        search::{stop_reasons, with_reasons, NarrowHint},
        workspace_walk::WalkStop,
    };

    let narrow = NarrowHint("the pattern or path");
    let counts = "no files matching '*.rs' under .".to_string();
    let cases = [
        (
            WalkStop::Cancelled,
            "no files matching '*.rs' under . (cancelled)",
        ),
        (
            WalkStop::EntryLimit,
            "no files matching '*.rs' under . (scan limit reached; narrow the pattern or path)",
        ),
        (
            WalkStop::Deadline,
            "no files matching '*.rs' under . (time limit reached)",
        ),
        (
            WalkStop::ResultLimit,
            "no files matching '*.rs' under . (result limit reached; narrow the pattern or path)",
        ),
    ];
    for (stop, expected) in cases {
        let reasons = stop_reasons(stop, /*per_file_truncated*/ 0);
        assert_eq!(with_reasons(counts.clone(), &reasons, narrow), expected);
    }
    // A finished walk keeps the plain empty message.
    let reasons = stop_reasons(WalkStop::Completed, /*per_file_truncated*/ 0);
    assert_eq!(
        with_reasons(counts, &reasons, narrow),
        "no files matching '*.rs' under ."
    );
}
