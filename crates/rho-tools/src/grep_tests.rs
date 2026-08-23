use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::{grep_workspace, truncate_chars, GrepRequest, MAX_FILE_BYTES};
use crate::{
    file_view::FileViewStyle,
    hashline::compute_file_hash,
    tool::{compact_display_path, resolve_path, ToolError},
};

/// Runs grep the way the SDK adapter does: parse, resolve the root against the
/// workspace, then walk.
fn call_grep(dir: &TempDir, args: serde_json::Value) -> Result<String, ToolError> {
    let request = GrepRequest::from_arguments(args)?;
    let root = resolve_path(dir.path(), &request.path);
    let display = compact_display_path(dir.path(), &request.path);
    grep_workspace(
        &root,
        &display,
        &request,
        &|| false,
        FileViewStyle::Hashline,
    )
}

fn write(dir: &TempDir, relative: &str, content: &str) {
    let path = dir.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

// Covers: max_per_file must suppress extras without dropping the file
// Owner: pure unit (grep limits)
#[test]
fn max_per_file_suppresses_extra_hits() {
    let dir = TempDir::new().unwrap();
    let body = "hit\nhit\nhit\n";
    write(&dir, "hits.rs", body);
    let tag = compute_file_hash(body);

    let content = call_grep(&dir, json!({"pattern": "hit", "max_per_file": 1})).unwrap();

    assert_eq!(
        content,
        format!(
            "\
[hits.rs#{tag}]
1 | hit
... +2 more in this file

1 matches shown (3 total) in 1 files (1 files truncated by max_per_file; raise max_per_file or narrow the pattern)"
        )
    );
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
fn max_results_caps_content_output() {
    let dir = TempDir::new().unwrap();
    let a = "match one\n";
    write(&dir, "a.txt", a);
    write(&dir, "b.txt", "match two\n");
    let tag = compute_file_hash(a);

    let content = call_grep(&dir, json!({"pattern": "match", "max_results": 1})).unwrap();
    assert_eq!(
        content,
        format!(
            "\
[a.txt#{tag}]
1 | match one

1 matches in 1 files (result limit reached; narrow the pattern, path, or glob)"
        )
    );
}

#[test]
fn max_results_splits_a_file_and_reports_the_remainder() {
    let dir = TempDir::new().unwrap();
    let body = "hit\nhit\nhit\n";
    write(&dir, "a.txt", body);
    let tag = compute_file_hash(body);

    let content = call_grep(&dir, json!({"pattern": "hit", "max_results": 2})).unwrap();
    assert_eq!(
        content,
        format!(
            "\
[a.txt#{tag}]
1 | hit
2 | hit
... +1 more in this file

2 matches shown (3 total) in 1 files (result limit reached; narrow the pattern, path, or glob)"
        )
    );
}

// Covers: binary and oversized files must not be scanned as text
// Owner: pure unit (grep safety)
#[test]
fn skips_binary_and_oversized_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("bin.dat"), b"a\0b\nx\n").unwrap();
    let oversized = dir.path().join("huge.txt");
    {
        let file = std::fs::File::create(&oversized).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
    }
    write(&dir, "ok.txt", "needle\n");

    let content = call_grep(&dir, json!({"pattern": "needle|x"})).unwrap();
    assert!(content.contains("ok.txt"), "{content}");
    assert!(!content.contains("bin.dat"), "{content}");
    assert!(!content.contains("huge.txt"), "{content}");
}

// Covers: files_with_matches must still honor MAX_FILE_BYTES when the first line hits
// Owner: pure unit (grep safety)
#[test]
fn files_with_matches_excludes_oversized_files_that_match_early() {
    let dir = TempDir::new().unwrap();
    let huge = dir.path().join("huge.txt");
    std::fs::write(&huge, b"needle\n").unwrap();
    {
        let file = std::fs::OpenOptions::new().write(true).open(&huge).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
    }
    write(&dir, "ok.txt", "needle\n");

    let content = call_grep(
        &dir,
        json!({"pattern": "needle", "output_mode": "files_with_matches"}),
    )
    .unwrap();
    assert!(content.contains("ok.txt"), "{content}");
    assert!(!content.contains("huge.txt"), "{content}");
}

// Covers: files_with_matches may keep a first-line hit when later bytes are not UTF-8
// Owner: pure unit (grep encoding)
#[test]
fn files_with_matches_accepts_a_first_line_hit_before_invalid_utf8() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("mixed.txt"), b"needle\n\xff\n").unwrap();

    let files = call_grep(
        &dir,
        json!({"pattern": "needle", "output_mode": "files_with_matches"}),
    )
    .unwrap();
    assert!(files.contains("mixed.txt"), "{files}");

    let content = call_grep(&dir, json!({"pattern": "needle"})).unwrap();
    assert!(
        !content.contains("mixed.txt"),
        "content mode still rejects the file when hashing later invalid UTF-8: {content}"
    );
}

// Covers: gitignore and hidden defaults are security boundaries
// Owner: pure unit (grep path policy)
#[test]
fn honors_gitignore_and_include_hidden() {
    let dir = TempDir::new().unwrap();
    write(&dir, ".gitignore", "secret.txt\n");
    write(&dir, "secret.txt", "needle\n");
    write(&dir, "visible.txt", "needle\n");
    write(&dir, ".hidden/dot.txt", "needle\n");

    let default = call_grep(&dir, json!({"pattern": "needle"})).unwrap();
    assert!(default.contains("visible.txt"), "{default}");
    assert!(!default.contains("secret.txt"), "{default}");
    assert!(!default.contains(".hidden"), "{default}");

    let hidden = call_grep(&dir, json!({"pattern": "needle", "include_hidden": true})).unwrap();
    assert!(hidden.contains(".hidden/dot.txt"), "{hidden}");
    assert!(!hidden.contains("secret.txt"), "{hidden}");
}

// Covers: long match lines truncate on a char boundary, not mid-codepoint
// Owner: pure unit (grep safety)
#[test]
fn truncates_long_match_lines_at_char_boundary() {
    let dir = TempDir::new().unwrap();
    let long = format!("{}é{}", "a".repeat(199), "b".repeat(20));
    write(&dir, "long.txt", &format!("{long}\n"));

    let content = call_grep(&dir, json!({"pattern": "a"})).unwrap();
    let line = content
        .lines()
        .find(|line| line.contains(" | ") && !line.starts_with('['))
        .unwrap();
    // Preview body: `1 | <text>…` - not hashline `N:text`
    let text = line.split_once(" | ").unwrap().1;
    assert!(text.ends_with('…'), "{line}");
    assert_eq!(text.chars().count(), 201, "{text}");
}

// Covers: cancel mid-walk must not look like a normal empty result without marker
// Owner: pure unit (grep cancellation)
#[test]
fn cancellation_stops_the_walk_and_is_reported() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "needle\n");
    let request = GrepRequest::from_arguments(json!({"pattern": "needle"})).unwrap();
    let out = grep_workspace(dir.path(), ".", &request, &|| true, FileViewStyle::Hashline).unwrap();
    assert_eq!(out, "no matches for 'needle' under . (cancelled)");
}

// Covers: content mode must mint full-file tags and line numbers for edit anchors
// Owner: pure unit (grep hashline)
#[test]
fn content_mode_mints_file_tags_and_preview_lines() {
    let dir = TempDir::new().unwrap();
    let body = "alpha\nfind me\nbeta\n";
    write(&dir, "src/lib.rs", body);
    let tag = compute_file_hash(body);
    let content = call_grep(&dir, json!({"pattern": "find me"})).unwrap();
    assert!(
        content.contains(&format!("[src/lib.rs#{tag}]")),
        "{content}"
    );
    // Anchor + preview shape, not hashline body `N:text`
    assert!(content.contains("2 | find me"), "{content}");
    assert!(
        !content.contains("2:find me"),
        "must not emit hashline body rows: {content}"
    );
}

// Covers: content-mode previews keep source indentation for readability
// Owner: pure unit (grep display)
#[test]
fn preserves_indentation_in_match_preview() {
    let dir = TempDir::new().unwrap();
    let body = "fn main() {\n    println!(\"hi\");\n}\n";
    write(&dir, "main.rs", body);
    let tag = compute_file_hash(body);
    let content = call_grep(&dir, json!({"pattern": "println"})).unwrap();
    assert!(content.contains(&format!("[main.rs#{tag}]")), "{content}");
    assert!(
        content.contains("2 |     println!(\"hi\");"),
        "indentation stripped: {content}"
    );
    assert!(
        !content.contains("2:    println!(\"hi\");"),
        "must not emit hashline body rows: {content}"
    );
}

// Covers: narrowed path= must emit workspace-relative chain headers edit accepts
// Owner: pure unit (grep hashline path contract)
#[test]
fn content_mode_headers_are_workspace_relative_under_narrowed_path() {
    let dir = TempDir::new().unwrap();
    let body = "anchor line\n";
    write(&dir, "src/nested.txt", body);
    let tag = compute_file_hash(body);
    let content = call_grep(&dir, json!({"pattern": "anchor", "path": "src"})).unwrap();
    assert!(
        content.contains(&format!("[src/nested.txt#{tag}]")),
        "expected workspace-relative header, got: {content}"
    );
    assert!(
        !content.contains(&format!("[nested.txt#{tag}]")),
        "must not emit walk-root-relative header: {content}"
    );
}

// Covers: non-hashline grep content mode must not mint full-file tags
// Owner: pure unit (grep hashline)
#[test]
fn content_mode_omits_file_tags_when_disabled() {
    let dir = TempDir::new().unwrap();
    let body = "find me\n";
    write(&dir, "src/lib.rs", body);
    let request = GrepRequest::from_arguments(json!({"pattern": "find me"})).unwrap();
    let root = resolve_path(dir.path(), &request.path);
    let display = compact_display_path(dir.path(), &request.path);
    let content = grep_workspace(
        &root,
        &display,
        &request,
        &|| false,
        FileViewStyle::Numbered,
    )
    .unwrap();
    assert!(content.contains("src/lib.rs"), "{content}");
    assert!(!content.contains("[src/lib.rs#"), "{content}");
    assert!(content.contains("1 | find me"), "{content}");
}

// Covers: truncate_chars bounds multibyte UTF-8 strings at character boundaries
// Owner: pure unit (grep truncate_chars)
#[test]
fn truncate_chars_respects_char_boundaries() {
    assert_eq!(truncate_chars("hello", 10), "hello");
    assert_eq!(truncate_chars("hello world", 5), "hello…");
    assert_eq!(truncate_chars("🦀🦀🦀🦀", 2), "🦀🦀…");
    assert_eq!(truncate_chars("café", 3), "caf…");
    assert_eq!(truncate_chars("", 5), "");
}

// Covers: parallel grep must emit the same path-ordered prefix every run
// Owner: pure unit (grep determinism)
#[test]
fn parallel_grep_is_deterministic_across_runs() {
    let dir = TempDir::new().unwrap();
    for i in 0..40 {
        write(&dir, &format!("f{i:02}.txt"), &format!("needle {i}\n"));
    }
    let args = json!({"pattern": "needle", "output_mode": "files_with_matches"});
    let first = call_grep(&dir, args.clone()).unwrap();
    let second = call_grep(&dir, args).unwrap();
    assert_eq!(first, second);
    let files: Vec<_> = first
        .lines()
        .take_while(|line| !line.is_empty() && !line.ends_with("files"))
        .collect();
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted);
}

// Covers: files_with_matches / count still skip binaries and respect gitignore
// Owner: pure unit (grep early-exit modes)
#[test]
fn early_exit_modes_skip_binary_and_gitignore() {
    let dir = TempDir::new().unwrap();
    write(&dir, ".gitignore", "secret.txt\n");
    write(&dir, "secret.txt", "needle\n");
    write(&dir, "visible.txt", "needle\n");
    std::fs::write(dir.path().join("bin.dat"), b"needle\0x\n").unwrap();

    for mode in ["files_with_matches", "count"] {
        let content = call_grep(&dir, json!({"pattern": "needle", "output_mode": mode})).unwrap();
        assert!(content.contains("visible.txt"), "{mode}: {content}");
        assert!(!content.contains("secret.txt"), "{mode}: {content}");
        assert!(!content.contains("bin.dat"), "{mode}: {content}");
    }
}

// Covers: streaming content scan must mint the same tag as full-file hashing
// Owner: pure unit (grep hashline)
#[test]
fn streaming_content_tags_match_full_file_hash() {
    let dir = TempDir::new().unwrap();
    let cases = [
        ("crlf.txt", "hit\r\nmiss\r\n"),
        ("trailing.txt", "hit\n"),
        ("no_nl.txt", "hit"),
        ("blank_then_hit.txt", "\nhit\n"),
    ];
    for (name, body) in cases {
        write(&dir, name, body);
        let tag = compute_file_hash(body);
        let content = call_grep(&dir, json!({"pattern": "hit", "glob": name})).unwrap();
        assert!(
            content.contains(&format!("[{name}#{tag}]")),
            "{name}: {content}"
        );
    }
}

// Covers: files_with_matches can stop after the first hit in a file
// Owner: pure unit (grep output modes)
#[test]
fn files_with_matches_lists_each_file_once() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "hit\nhit\n");
    write(&dir, "b.txt", "miss\n");
    write(&dir, "c.txt", "hit\n");
    let content = call_grep(
        &dir,
        json!({"pattern": "hit", "output_mode": "files_with_matches"}),
    )
    .unwrap();
    assert_eq!(
        content,
        "\
a.txt
c.txt

2 files"
    );
}
