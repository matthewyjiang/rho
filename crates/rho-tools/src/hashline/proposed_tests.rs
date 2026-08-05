use pretty_assertions::assert_eq;

use super::*;
use crate::hashline::format::compute_file_hash;

// Covers: proposal cards must show op summaries and PUT bodies from the document
// alone so streaming does not need the target file
// Owner: hashline proposed
#[test]
fn projects_mixed_ops_into_document_rows() {
    let proposed = proposed_edit(
        r#"[src/a.rs#A1B2]
PUT 2.=3:
+TWO
+THREE
CUT 5.=5
PUT >6:
+tail
"#,
    );
    assert_eq!(proposed.files.len(), 1);
    let file = &proposed.files[0];
    assert_eq!(file.path, "src/a.rs");
    assert_eq!(file.added_lines, 3);
    assert_eq!(file.removed_lines, 3);
    assert!(file.document_only);
    assert!(!file.pure_delete);
    assert_eq!(
        file.rows,
        vec![
            ProposedRow::Summary("PUT 2.=3".into()),
            ProposedRow::Added("TWO".into()),
            ProposedRow::Added("THREE".into()),
            ProposedRow::Summary("CUT 5".into()),
            ProposedRow::Summary("PUT >6".into()),
            ProposedRow::Added("tail".into()),
        ]
    );
    assert!(!proposed.truncated);
    assert!(proposed.document_only);
}

// Covers: incomplete streamed documents still yield path counts and partial rows
// Owner: hashline proposed
#[test]
fn projects_incomplete_documents() {
    let proposed =
        proposed_edit("[a.rs#ABCD]\nPUT 1.=3:\n+one\n+two\nCUT 8.=9\n[b.rs#ABCD]\nPUT 2:\n+pa");
    assert_eq!(
        proposed_sections("[a.rs#ABCD]\nPUT 1.=3:\n+one\n+two\nCUT 8.=9\n[b.rs#ABCD]\nPUT 2:\n+pa"),
        vec![
            ProposedSection {
                path: "a.rs".into(),
                added_lines: 2,
                removed_lines: 5,
            },
            ProposedSection {
                path: "b.rs".into(),
                added_lines: 1,
                removed_lines: 1,
            },
        ]
    );
    // PUT summary + 2 bodies + CUT summary; PUT summary + body (partial stream)
    assert_eq!(proposed.files[0].rows.len(), 4);
    assert_eq!(proposed.files[1].rows.len(), 2);
    assert_eq!(
        proposed.files[0].rows[0],
        ProposedRow::Summary("PUT 1.=3".into())
    );
    assert_eq!(
        proposed.files[0].rows[3],
        ProposedRow::Summary("CUT 8.=9".into())
    );
}

// Covers: pure CUT sections are flagged as pure_delete for DiffCardChange
// Owner: hashline proposed
#[test]
fn marks_pure_delete_sections() {
    let proposed = proposed_edit("[gone.txt#AAAA]\nCUT 1.=3\n");
    assert!(proposed.files[0].pure_delete);
    assert_eq!(proposed.files[0].added_lines, 0);
    assert_eq!(proposed.files[0].removed_lines, 3);
}

// Covers: planned_edit applies against live text and surfaces removed lines
// Owner: hashline proposed
#[test]
fn plans_live_content_diff_with_removals() {
    let original = "alpha\nbeta\ngamma\n";
    let tag = compute_file_hash(original);
    let input = format!("[sample.txt#{tag}]\nPUT 2.=2:\n+BETA\nCUT 3.=3\n");
    let planned = planned_edit(&input, |path| {
        assert_eq!(path, "sample.txt");
        Some(original.to_string())
    });
    assert_eq!(planned.files.len(), 1);
    let file = &planned.files[0];
    assert!(!file.document_only);
    assert!(!planned.document_only);
    assert!(
        file.rows
            .iter()
            .any(|row| matches!(row, ProposedRow::Removed(text) if text == "beta")),
        "expected removed beta: {:?}",
        file.rows
    );
    assert!(
        file.rows
            .iter()
            .any(|row| matches!(row, ProposedRow::Removed(text) if text == "gamma")),
        "expected removed gamma: {:?}",
        file.rows
    );
    assert!(
        file.rows
            .iter()
            .any(|row| matches!(row, ProposedRow::Added(text) if text == "BETA")),
        "expected added BETA: {:?}",
        file.rows
    );
    assert!(file.removed_lines >= 2);
    assert!(file.added_lines >= 1);
}

// Covers: planned_edit falls back to document rows when the file is missing
// Owner: hashline proposed
#[test]
fn planned_edit_falls_back_without_live_file() {
    let planned = planned_edit("[missing.txt#AAAA]\nPUT 1.=1:\n+x\n", |_| None);
    assert!(planned.document_only);
    assert!(planned.files.iter().all(|f| f.document_only));
    assert!(planned
        .files
        .iter()
        .flat_map(|f| &f.rows)
        .all(|row| !matches!(row, ProposedRow::Removed(_))));
}
