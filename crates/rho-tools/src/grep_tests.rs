use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::{grep_workspace, GrepRequest, MAX_FILE_BYTES};
use crate::tool::{compact_display_path, resolve_path, ToolError};

/// Runs grep the way the SDK adapter does: parse, resolve the root against the
/// workspace, then walk.
fn call_grep(dir: &TempDir, args: serde_json::Value) -> Result<String, ToolError> {
    let request = GrepRequest::from_arguments(args)?;
    let root = resolve_path(dir.path(), &request.path);
    let display = compact_display_path(dir.path(), &request.path);
    grep_workspace(&root, &display, &request, &|| false)
}

fn write(dir: &TempDir, relative: &str, content: &str) {
    let path = dir.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn content_mode_groups_matches_by_file() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "crates/rho/src/agent/parser.rs",
        "fn other() {}\nlet capability = ToolCapability::parse(name.clone());\nfn mid() {}\nToolCapability::parse(name.to_string()),\n",
    );
    write(
        &dir,
        "crates/rho/src/agent/definition.rs",
        "pub fn parse(name: String) -> Self {\n",
    );

    let content = call_grep(&dir, json!({"pattern": "parse", "path": "crates/rho"})).unwrap();

    assert_eq!(
        content,
        "\
src/agent/definition.rs
  1: pub fn parse(name: String) -> Self {
src/agent/parser.rs
  2: let capability = ToolCapability::parse(name.clone());
  4: ToolCapability::parse(name.to_string()),

3 matches in 2 files"
    );
}

#[test]
fn normalizes_match_text_whitespace() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.rs", "\t  foo\t\tbar   \n");

    let content = call_grep(&dir, json!({"pattern": "foo"})).unwrap();
    assert!(content.contains("  1: foo bar\n"), "{content}");
}

#[test]
fn max_per_file_suppresses_extra_hits() {
    let dir = TempDir::new().unwrap();
    write(&dir, "hits.rs", "hit\nhit\nhit\n");

    let content = call_grep(&dir, json!({"pattern": "hit", "max_per_file": 1})).unwrap();

    assert_eq!(
        content,
        "\
hits.rs
  1: hit
  ... +2 more in this file

1 matches shown (3 total) in 1 files (1 files truncated by max_per_file; raise max_per_file or narrow the pattern)"
    );
}

#[test]
fn glob_filter_excludes_non_matching_files() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.rs", "needle\n");
    write(&dir, "a.txt", "needle\n");

    let content = call_grep(&dir, json!({"pattern": "needle", "glob": "*.rs"})).unwrap();
    assert!(content.contains("a.rs\n"), "{content}");
    assert!(!content.contains("a.txt"), "{content}");
    assert!(content.contains("1 matches in 1 files"), "{content}");
}

#[test]
fn literal_mode_escapes_metacharacters() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "a.b\naxb\n");

    let literal = call_grep(&dir, json!({"pattern": "a.b", "literal": true})).unwrap();
    assert!(literal.contains("a.b"), "{literal}");
    assert!(!literal.contains("axb"), "{literal}");
    assert!(literal.contains("1 matches in 1 files"), "{literal}");

    let regex = call_grep(&dir, json!({"pattern": "a.b", "literal": false})).unwrap();
    assert!(regex.contains("2 matches in 1 files"), "{regex}");
}

#[test]
fn case_insensitive_search() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "FOO\n");

    let content = call_grep(&dir, json!({"pattern": "foo", "case_sensitive": false})).unwrap();
    assert!(content.contains("  1: FOO\n"), "{content}");
}

#[test]
fn invalid_regex_and_output_mode_error() {
    let dir = TempDir::new().unwrap();
    let err = call_grep(&dir, json!({"pattern": "("})).unwrap_err();
    match err {
        ToolError::Message(message) => assert!(message.contains("invalid pattern"), "{message}"),
        other => panic!("unexpected {other:?}"),
    }

    let err = call_grep(&dir, json!({"pattern": "x", "output_mode": "nope"})).unwrap_err();
    match err {
        ToolError::Message(message) => {
            assert!(message.contains("invalid output_mode"), "{message}")
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn no_matches_returns_ok_message() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "hello\n");
    let content = call_grep(&dir, json!({"pattern": "Foo", "path": "."})).unwrap();
    assert_eq!(content, "no matches for 'Foo' under .");
}

#[test]
fn max_results_caps_content_output() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "match one\n");
    write(&dir, "b.txt", "match two\n");

    let content = call_grep(&dir, json!({"pattern": "match", "max_results": 1})).unwrap();
    assert_eq!(
        content,
        "\
a.txt
  1: match one

1 matches in 1 files (result limit reached; narrow the pattern, path, or glob)"
    );
}

#[test]
fn max_results_splits_a_file_and_reports_the_remainder() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "hit\nhit\nhit\n");

    let content = call_grep(&dir, json!({"pattern": "hit", "max_results": 2})).unwrap();
    assert_eq!(
        content,
        "\
a.txt
  1: hit
  2: hit
  ... +1 more in this file

2 matches shown (3 total) in 1 files (result limit reached; narrow the pattern, path, or glob)"
    );
}

#[test]
fn files_with_matches_lists_paths_only() {
    let dir = TempDir::new().unwrap();
    write(&dir, "b.txt", "x\n");
    write(&dir, "a.txt", "x\n");

    let content = call_grep(
        &dir,
        json!({"pattern": "x", "output_mode": "files_with_matches"}),
    )
    .unwrap();
    assert_eq!(
        content,
        "\
a.txt
b.txt

2 files"
    );
}

#[test]
fn count_mode_emits_path_counts() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "x\nx\n");
    write(&dir, "b.txt", "x\n");

    let content = call_grep(&dir, json!({"pattern": "x", "output_mode": "count"})).unwrap();
    assert_eq!(
        content,
        "\
a.txt:2
b.txt:1

3 matches in 2 files"
    );
}

#[test]
fn skips_binary_and_oversized_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("bin.dat"), b"a\0b\nx\n").unwrap();
    // Sparse/truncated file: large enough to exceed the byte cap without writing
    // multi-megabyte contents into the fixture.
    let oversized = dir.path().join("huge.txt");
    {
        let file = std::fs::File::create(&oversized).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
    }
    write(&dir, "ok.txt", "needle\n");

    let content = call_grep(&dir, json!({"pattern": "needle|x"})).unwrap();
    assert!(content.contains("ok.txt\n"), "{content}");
    assert!(!content.contains("bin.dat"), "{content}");
    assert!(!content.contains("huge.txt"), "{content}");
}

#[test]
fn honors_gitignore_and_include_hidden() {
    let dir = TempDir::new().unwrap();
    write(&dir, ".gitignore", "secret.txt\n");
    write(&dir, "secret.txt", "needle\n");
    write(&dir, "visible.txt", "needle\n");
    write(&dir, ".hidden/dot.txt", "needle\n");

    let default = call_grep(&dir, json!({"pattern": "needle"})).unwrap();
    assert!(default.contains("visible.txt\n"), "{default}");
    assert!(!default.contains("secret.txt"), "{default}");
    assert!(!default.contains(".hidden"), "{default}");

    let hidden = call_grep(&dir, json!({"pattern": "needle", "include_hidden": true})).unwrap();
    assert!(hidden.contains(".hidden/dot.txt\n"), "{hidden}");
    assert!(!hidden.contains("secret.txt"), "{hidden}");
}

#[test]
fn truncates_long_match_lines_at_char_boundary() {
    let dir = TempDir::new().unwrap();
    let long = format!("{}é{}", "a".repeat(199), "b".repeat(20));
    write(&dir, "long.txt", &format!("{long}\n"));

    let content = call_grep(&dir, json!({"pattern": "a"})).unwrap();
    let line = content
        .lines()
        .find(|line| line.starts_with("  1: "))
        .unwrap();
    assert!(line.ends_with('…'), "{line}");
    // 200 kept characters plus the ellipsis.
    let text = &line["  1: ".len()..];
    assert_eq!(text.chars().count(), 201, "{text}");
}

#[test]
fn request_path_defaults_to_dot() {
    let dir = TempDir::new().unwrap();
    write(&dir, "hit.txt", "x\n");
    let request = GrepRequest::from_arguments(json!({"pattern": "x"})).unwrap();
    assert_eq!(request.path, ".");
    let out = grep_workspace(dir.path(), ".", &request, &|| false).unwrap();
    assert_eq!(
        out,
        "\
hit.txt
  1: x

1 matches in 1 files"
    );
}

#[test]
fn cancellation_stops_the_walk_and_is_reported() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "needle\n");
    let request = GrepRequest::from_arguments(json!({"pattern": "needle"})).unwrap();
    let out = grep_workspace(dir.path(), ".", &request, &|| true).unwrap();
    assert_eq!(out, "no matches for 'needle' under . (cancelled)");
}
