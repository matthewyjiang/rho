//! Document-only projection of a hashline edit for presentation cards.
//!
//! Rows come from the edit document alone so streaming and approval cards never
//! need the target file on disk. Added lines use PUT bodies; removed lines use
//! original line numbers from CUT/replace ranges (body text is not in the
//! document, so remove rows carry line numbers only).

use crate::tool_card::{DiffRow, DiffRowKind};

use super::parser::{parse_lenient, Op, Section};

/// Maximum rendered body rows for a proposed card, including multi-file spend.
const MAX_PROPOSED_DIFF_ROWS: usize = 1_000;
/// Maximum file sections retained for a proposed card.
const MAX_PROPOSED_DIFF_FILES: usize = 100;

/// Best-effort proposed edit grouped by file section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProposedEdit {
    pub files: Vec<ProposedEditFile>,
    /// True when file sections or body rows were dropped to stay in budget.
    pub truncated: bool,
}

/// One file section as far as the document has streamed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedEditFile {
    pub path: String,
    pub added_lines: u64,
    pub removed_lines: u64,
    pub rows: Vec<DiffRow>,
}

/// Path and line counts only, for metadata and path lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedSection {
    pub path: String,
    pub added_lines: u64,
    pub removed_lines: u64,
}

/// Project a possibly incomplete document into display-ready file cards.
pub fn proposed_edit(input: &str) -> ProposedEdit {
    let mut proposed = ProposedEdit::default();
    let mut retained_body_rows = 0usize;
    let mut body_overflow = false;
    let mut files_truncated = false;

    for section in parse_lenient(input) {
        if proposed.files.len() >= MAX_PROPOSED_DIFF_FILES {
            files_truncated = true;
            break;
        }
        let mut file = ProposedEditFile {
            path: section.path,
            added_lines: 0,
            removed_lines: 0,
            rows: Vec::new(),
        };
        for op in &section.ops {
            project_op(op, &mut file, &mut retained_body_rows, &mut body_overflow);
        }
        proposed.files.push(file);
    }

    proposed.truncated = body_overflow || files_truncated;
    proposed
}

/// Path and count summary of a possibly incomplete document.
///
/// Cheap projection: does not allocate diff rows. Use [`proposed_edit`] when
/// the caller needs card body content.
pub fn proposed_sections(input: &str) -> Vec<ProposedSection> {
    parse_lenient(input)
        .into_iter()
        .map(|section| {
            let (added_lines, removed_lines) = count_section_lines(&section);
            ProposedSection {
                path: section.path,
                added_lines,
                removed_lines,
            }
        })
        .collect()
}

fn count_section_lines(section: &Section) -> (u64, u64) {
    let mut added = 0u64;
    let mut removed = 0u64;
    for op in &section.ops {
        match op {
            Op::Replace { start, end, body } => {
                removed += (*end - *start + 1) as u64;
                added += body.len() as u64;
            }
            Op::Delete { start, end } => {
                removed += (*end - *start + 1) as u64;
            }
            Op::InsertBefore { body, .. } | Op::InsertAfter { body, .. } => {
                added += body.len() as u64;
            }
        }
    }
    (added, removed)
}

fn project_op(
    op: &Op,
    file: &mut ProposedEditFile,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    match op {
        Op::Replace { start, end, body } => {
            file.removed_lines += (*end - *start + 1) as u64;
            file.added_lines += body.len() as u64;
            push_removed_range(file, *start, *end, retained_body_rows, body_overflow);
            for line in body {
                push_row(
                    file,
                    DiffRowKind::Added,
                    None,
                    line,
                    retained_body_rows,
                    body_overflow,
                );
            }
        }
        Op::Delete { start, end } => {
            file.removed_lines += (*end - *start + 1) as u64;
            push_removed_range(file, *start, *end, retained_body_rows, body_overflow);
        }
        Op::InsertBefore { body, .. } | Op::InsertAfter { body, .. } => {
            file.added_lines += body.len() as u64;
            for text in body {
                push_row(
                    file,
                    DiffRowKind::Added,
                    None,
                    text,
                    retained_body_rows,
                    body_overflow,
                );
            }
        }
    }
}

fn push_removed_range(
    file: &mut ProposedEditFile,
    start: usize,
    end: usize,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    for line in start..=end {
        push_row(
            file,
            DiffRowKind::Removed,
            u32::try_from(line).ok(),
            "",
            retained_body_rows,
            body_overflow,
        );
    }
}

fn push_row(
    file: &mut ProposedEditFile,
    kind: DiffRowKind,
    line: Option<u32>,
    text: &str,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    if *retained_body_rows >= MAX_PROPOSED_DIFF_ROWS {
        *body_overflow = true;
        return;
    }
    file.rows.push(DiffRow::new(kind, line, text));
    *retained_body_rows += 1;
}

#[cfg(test)]
#[path = "proposed_tests.rs"]
mod tests;
