use pretty_assertions::assert_eq;

use super::*;
use crate::tool_card::DiffRowKind;

// Covers: proposal cards must show added bodies and removed line anchors from the
// document alone so approval does not need the target file
// Owner: hashline proposed
#[test]
fn projects_mixed_ops_into_diff_rows() {
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
    assert_eq!(
        file.rows,
        vec![
            DiffRow::new(DiffRowKind::Removed, Some(2), ""),
            DiffRow::new(DiffRowKind::Removed, Some(3), ""),
            DiffRow::new(DiffRowKind::Added, None, "TWO"),
            DiffRow::new(DiffRowKind::Added, None, "THREE"),
            DiffRow::new(DiffRowKind::Removed, Some(5), ""),
            DiffRow::new(DiffRowKind::Added, None, "tail"),
        ]
    );
    assert!(!proposed.truncated);
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
    assert_eq!(proposed.files[0].rows.len(), 7);
    assert_eq!(proposed.files[1].rows.len(), 2);
}
