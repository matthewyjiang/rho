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

// Covers: gitignore and hidden defaults are security boundaries
// Owner: pure unit (glob path policy)
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

// Covers: cancel must not look like a normal empty match set
// Owner: pure unit (glob cancellation)
#[test]
fn cancellation_is_reported_rather_than_read_as_no_matches() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.rs", "");
    let request = GlobRequest::from_arguments(json!({"pattern": "*.rs"})).unwrap();
    let out = glob_workspace(dir.path(), ".", &request, &|| true).unwrap();
    assert_eq!(out, "no files matching '*.rs' under . (cancelled)");
}

// Covers: empty results distinguish cancel / entry / deadline / result stops
// Owner: pure unit (glob stop reasons)
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
    let reasons = stop_reasons(WalkStop::Completed, /*per_file_truncated*/ 0);
    assert_eq!(
        with_reasons(counts, &reasons, narrow),
        "no files matching '*.rs' under ."
    );
}
