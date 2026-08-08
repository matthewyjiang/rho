use rho_tools::tool_card::{DiffRow, DiffRowKind, ToolFamily, ToolHeader};

use super::*;
use crate::tui::theme::{SyntaxRole, Theme};

#[test]
fn sizes_gutter_to_the_widest_line_number() {
    let rows = vec![
        DiffRow::new(DiffRowKind::Context, Some(9), "keep"),
        DiffRow::new(DiffRowKind::Added, Some(1204), "new"),
        DiffRow::new(DiffRowKind::File, None, "src/lib.rs"),
    ];

    assert_eq!(gutter_width(&rows), 4);
}

#[test]
fn drops_the_gutter_when_no_row_is_numbered() {
    let rows = vec![DiffRow::new(DiffRowKind::Added, None, "new")];

    assert_eq!(gutter_width(&rows), 0);
}

// Covers: single-file write/edit header path seeds highlighting
// Owner: pure unit (diff syntax fallback path)
#[test]
fn single_file_path_from_file_diff_header() {
    assert_eq!(
        single_file_path_from_header(
            ToolFamily::FileDiff,
            &ToolHeader::call("write", Some("src/lib.rs".into())),
        ),
        Some("src/lib.rs")
    );
    assert_eq!(
        single_file_path_from_header(ToolFamily::FileCommand, &ToolHeader::call("diff", None)),
        None
    );
}

// Covers: rust add/remove rows pick up keyword roles after a File header
// Owner: pure unit (diff syntax highlighting)
#[test]
fn highlights_rust_tokens_after_file_row() {
    let mut syntax = DiffSyntax::new(None);
    let file = DiffRow::new(DiffRowKind::File, None, "src/main.rs");
    assert!(syntax.paint_row(&file).is_none());

    let added = DiffRow::new(DiffRowKind::Added, Some(1), "let answer = 42; // note");
    let segments = syntax.paint_row(&added).expect("rust highlighter for .rs");

    let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(joined, "let answer = 42; // note");
    assert!(segments
        .iter()
        .any(|s| { s.text.contains("let") && s.role == Some(SyntaxRole::Keyword) }));
    assert!(segments
        .iter()
        .any(|s| { s.text.contains("42") && s.role == Some(SyntaxRole::Constant) }));
    // Plain tokens stay role-less so the painter can apply add/remove color.
    let plain = Theme::tool_diff_text(DiffRowKind::Added);
    assert!(segments
        .iter()
        .any(|s| s.role.is_none() && s.style(plain) == plain));
}

// Covers: /diff +++ headers switch language without a File row
// Owner: pure unit (diff header path observe)
#[test]
fn paints_from_plus_plus_plus_header_path() {
    let mut syntax = DiffSyntax::new(None);
    let header = DiffRow::new(DiffRowKind::Context, None, "+++ b/app.ts");
    assert!(syntax.paint_row(&header).is_none());
    let added = DiffRow::new(DiffRowKind::Added, None, "const x = 1;");
    let segments = syntax.paint_row(&added).expect("ts highlighter");
    assert!(segments
        .iter()
        .any(|s| { s.text.contains("const") && s.role == Some(SyntaxRole::Keyword) }));
}

// Covers: rename arrows and diff prefixes strip before language lookup
// Owner: pure unit (diff path normalization)
#[test]
fn normalize_diff_path_strips_noise() {
    assert_eq!(normalize_diff_path("old.rs → src/new.rs"), "src/new.rs");
    assert_eq!(normalize_diff_path("b/src/lib.rs"), "src/lib.rs");
    assert_eq!(normalize_diff_path("a/src/lib.rs"), "src/lib.rs");
}

// Covers: unified-diff headers yield highlight paths for /diff
// Owner: pure unit (diff header path parse)
#[test]
fn path_from_diff_header_line_reads_git_headers() {
    assert_eq!(
        path_from_diff_header_line("+++ b/crates/rho/src/lib.rs"),
        Some("crates/rho/src/lib.rs")
    );
    assert_eq!(
        path_from_diff_header_line("--- a/crates/rho/src/lib.rs"),
        Some("crates/rho/src/lib.rs")
    );
    assert_eq!(
        path_from_diff_header_line("diff --git a/foo.rs b/bar.rs"),
        Some("bar.rs")
    );
    assert_eq!(path_from_diff_header_line("+++ /dev/null"), None);
    assert_eq!(path_from_diff_header_line("+added line"), None);
}
