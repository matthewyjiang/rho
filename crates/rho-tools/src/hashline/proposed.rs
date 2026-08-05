//! Document and planned projections of a hashline edit for presentation cards.
//!
//! Streaming cards use [`proposed_edit`]: rows come from the edit document alone
//! so mid-stream previews never need the target file on disk.
//!
//! Approval / start cards use [`planned_edit`]: when a reader can supply live
//! file text, ops are applied in memory and rows become a real content diff
//! (removals included). Paths that cannot be planned fall back to the document
//! projection for that section only.

use super::{
    apply::apply_ops,
    parser::{parse_hashline, parse_lenient, Op, Section},
};
use crate::diff::unified_diff;

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
    /// True when at least one section used document-only rows because a live
    /// plan was unavailable (missing file, stale tag, apply error, or stream).
    pub document_only: bool,
}

/// One file section as far as the document has streamed in / been planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedEditFile {
    pub path: String,
    pub added_lines: u64,
    pub removed_lines: u64,
    /// True when this section only deletes lines (no adds).
    pub pure_delete: bool,
    /// True when rows are document projection rather than a live content diff.
    pub document_only: bool,
    pub rows: Vec<ProposedRow>,
}

/// One display row for a proposed / planned card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposedRow {
    /// Op locator / range summary (`PUT 2.=3`, `CUT 5`) — document projection.
    Summary(String),
    /// Added line (PUT body or planned +).
    Added(String),
    /// Removed line from a live plan (never invented for streaming).
    Removed(String),
    /// Unchanged context line from a live plan.
    Context(String),
}

/// Path and line counts only, for metadata and path lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedSection {
    pub path: String,
    pub added_lines: u64,
    pub removed_lines: u64,
}

/// Project a possibly incomplete document into display-ready file cards.
///
/// Document-only: never reads disk. Use for streaming argument previews.
pub fn proposed_edit(input: &str) -> ProposedEdit {
    let mut proposed = ProposedEdit {
        document_only: true,
        ..ProposedEdit::default()
    };
    let mut retained_body_rows = 0usize;
    let mut body_overflow = false;
    let mut files_truncated = false;

    for section in parse_lenient(input) {
        if proposed.files.len() >= MAX_PROPOSED_DIFF_FILES {
            files_truncated = true;
            break;
        }
        let file = project_section_document(section, &mut retained_body_rows, &mut body_overflow);
        proposed.files.push(file);
    }

    proposed.truncated = body_overflow || files_truncated;
    proposed
}

/// Plan a content diff against live file text when possible.
///
/// `read_path` receives each section path as written in the document and should
/// return UTF-8 file contents when the path is readable in the caller's
/// workspace. Sections that cannot be planned fall back to
/// [`proposed_edit`]-style document rows.
pub fn planned_edit(
    input: &str,
    mut read_path: impl FnMut(&str) -> Option<String>,
) -> ProposedEdit {
    let sections = match parse_hashline(input) {
        Ok(sections) => sections,
        // Incomplete / malformed documents stay on the streaming projection.
        Err(_) => return proposed_edit(input),
    };

    let mut proposed = ProposedEdit::default();
    let mut retained_body_rows = 0usize;
    let mut body_overflow = false;
    let mut files_truncated = false;

    for section in sections {
        if proposed.files.len() >= MAX_PROPOSED_DIFF_FILES {
            files_truncated = true;
            break;
        }
        let path = section.path.clone();
        let file = match read_path(&path) {
            Some(original) => match apply_ops(&original, &section.tag, section.ops.clone()) {
                Ok(outcome) => project_section_planned(
                    path,
                    &original,
                    &outcome.text,
                    &mut retained_body_rows,
                    &mut body_overflow,
                ),
                Err(_) => {
                    proposed.document_only = true;
                    project_section_document(
                        Section {
                            path,
                            tag: section.tag,
                            ops: section.ops,
                        },
                        &mut retained_body_rows,
                        &mut body_overflow,
                    )
                }
            },
            None => {
                proposed.document_only = true;
                project_section_document(
                    Section {
                        path,
                        tag: section.tag,
                        ops: section.ops,
                    },
                    &mut retained_body_rows,
                    &mut body_overflow,
                )
            }
        };
        proposed.files.push(file);
    }

    proposed.truncated = body_overflow || files_truncated;
    proposed
}

/// Path and count summary of a possibly incomplete document.
///
/// Cheap projection: does not allocate display rows. Use [`proposed_edit`] when
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

fn project_section_document(
    section: Section,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) -> ProposedEditFile {
    let mut file = ProposedEditFile {
        path: section.path,
        added_lines: 0,
        removed_lines: 0,
        pure_delete: false,
        document_only: true,
        rows: Vec::new(),
    };
    for op in &section.ops {
        project_op_document(op, &mut file, retained_body_rows, body_overflow);
    }
    file.pure_delete = file.added_lines == 0 && file.removed_lines > 0;
    file
}

fn project_section_planned(
    path: String,
    original: &str,
    updated: &str,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) -> ProposedEditFile {
    let diff = unified_diff(original, updated, &path, /*created*/ false);
    let mut file = ProposedEditFile {
        path,
        added_lines: 0,
        removed_lines: 0,
        pure_delete: false,
        document_only: false,
        rows: Vec::new(),
    };
    push_unified_diff_rows(&diff, &mut file, retained_body_rows, body_overflow);
    file.pure_delete = file.added_lines == 0 && file.removed_lines > 0;
    file
}

fn push_unified_diff_rows(
    diff: &str,
    file: &mut ProposedEditFile,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if !in_hunk {
            continue;
        }
        let Some(marker) = line.as_bytes().first().copied() else {
            continue;
        };
        let content = line.get(1..).unwrap_or_default();
        match marker {
            b'+' => {
                file.added_lines += 1;
                push_row(
                    file,
                    ProposedRow::Added(content.to_string()),
                    retained_body_rows,
                    body_overflow,
                );
            }
            b'-' => {
                file.removed_lines += 1;
                push_row(
                    file,
                    ProposedRow::Removed(content.to_string()),
                    retained_body_rows,
                    body_overflow,
                );
            }
            b' ' => {
                push_row(
                    file,
                    ProposedRow::Context(content.to_string()),
                    retained_body_rows,
                    body_overflow,
                );
            }
            _ => {}
        }
    }
}

fn project_op_document(
    op: &Op,
    file: &mut ProposedEditFile,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    match op {
        Op::Replace { start, end, body } => {
            file.removed_lines += (*end - *start + 1) as u64;
            file.added_lines += body.len() as u64;
            push_summary_and_body(
                file,
                format_put_range(*start, *end),
                body,
                retained_body_rows,
                body_overflow,
            );
        }
        Op::Delete { start, end } => {
            file.removed_lines += (*end - *start + 1) as u64;
            push_row(
                file,
                ProposedRow::Summary(format_cut_range(*start, *end)),
                retained_body_rows,
                body_overflow,
            );
        }
        Op::InsertBefore { line, body } => {
            file.added_lines += body.len() as u64;
            push_summary_and_body(
                file,
                format!("PUT <{line}"),
                body,
                retained_body_rows,
                body_overflow,
            );
        }
        Op::InsertAfter { line, body } => {
            file.added_lines += body.len() as u64;
            let summary = match line {
                Some(line) => format!("PUT >{line}"),
                None => "PUT >$".into(),
            };
            push_summary_and_body(file, summary, body, retained_body_rows, body_overflow);
        }
    }
}

fn push_summary_and_body(
    file: &mut ProposedEditFile,
    summary: String,
    body: &[String],
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    push_row(
        file,
        ProposedRow::Summary(summary),
        retained_body_rows,
        body_overflow,
    );
    for text in body {
        push_row(
            file,
            ProposedRow::Added(text.clone()),
            retained_body_rows,
            body_overflow,
        );
    }
}

fn format_put_range(start: usize, end: usize) -> String {
    if start == end {
        format!("PUT {start}")
    } else {
        format!("PUT {start}.={end}")
    }
}

fn format_cut_range(start: usize, end: usize) -> String {
    if start == end {
        format!("CUT {start}")
    } else {
        format!("CUT {start}.={end}")
    }
}

fn push_row(
    file: &mut ProposedEditFile,
    row: ProposedRow,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    if *retained_body_rows >= MAX_PROPOSED_DIFF_ROWS {
        *body_overflow = true;
        return;
    }
    file.rows.push(row);
    *retained_body_rows += 1;
}

#[cfg(test)]
#[path = "proposed_tests.rs"]
mod tests;
