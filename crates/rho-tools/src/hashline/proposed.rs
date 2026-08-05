//! Document and planned projections of a hashline edit for presentation cards.
//!
//! Streaming cards use [`proposed_edit`] ([`EditPreviewKind::Document`]): rows
//! come from the edit document alone so mid-stream previews never need disk and
//! never warn.
//!
//! Approval / start cards use [`planned_edit`] ([`EditPreviewKind::Planned`]):
//! when a reader can supply live file text, ops are applied in memory and rows
//! become a real content diff (removals included). Paths that cannot be planned
//! fall back to document rows and set `unverified` so hosts can warn once.

use super::{
    apply::apply_ops,
    format::{format_cut_locator, format_put_locator},
    parser::{parse_hashline, parse_lenient, Op, Section},
};
use crate::{
    diff::unified_diff,
    tool_card::{parse_unified_diff, DiffCardChange, DiffCardFile, DiffRow, DiffRowKind},
};

/// Maximum rendered body rows for a proposed card, including multi-file spend.
const MAX_PROPOSED_DIFF_ROWS: usize = 1_000;
/// Maximum file sections retained for a proposed card.
const MAX_PROPOSED_DIFF_FILES: usize = 100;

/// Notice when a planned card could not bind a section to live file text.
///
/// Hosts should use this constant for card-level facts so copy stays one place.
pub const EDIT_DOCUMENT_ONLY_NOTICE: &str =
    "document preview only - not verified against live file";

/// How an [`EditPreview`] was produced and whether hosts should warn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EditPreviewKind {
    /// Document projection only (streaming). Quiet: never warn.
    #[default]
    Document,
    /// Dry-run against live files when possible.
    Planned {
        /// True when at least one path fell back to document rows.
        unverified: bool,
    },
}

impl EditPreviewKind {
    /// Whether hosts should surface an unverified/document-only warning.
    pub fn warns_unverified(&self) -> bool {
        matches!(self, Self::Planned { unverified: true })
    }
}

/// Best-effort edit preview ready for FileDiff cards.
///
/// Emits existing [`DiffCardFile`] values directly so presenters do not map a
/// parallel DTO graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditPreview {
    pub files: Vec<DiffCardFile>,
    /// True when file sections or body rows were dropped to stay in budget.
    pub truncated: bool,
    /// Projection mode. Stream uses [`EditPreviewKind::Document`]; approval
    /// uses [`EditPreviewKind::Planned`].
    pub kind: EditPreviewKind,
}

impl EditPreview {
    /// Paths in document order (cheap helper for headers and path lists).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|file| file.path.as_str())
    }

    /// Whether hosts should surface the unverified notice on the card.
    pub fn warns_unverified(&self) -> bool {
        self.kind.warns_unverified()
    }
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
/// Document-only: never reads disk, never warns. Use for streaming previews.
pub fn proposed_edit(input: &str) -> EditPreview {
    let mut preview = EditPreview {
        kind: EditPreviewKind::Document,
        ..EditPreview::default()
    };
    let mut retained_body_rows = 0usize;
    let mut body_overflow = false;
    let mut files_truncated = false;

    for section in parse_lenient(input) {
        if preview.files.len() >= MAX_PROPOSED_DIFF_FILES {
            files_truncated = true;
            break;
        }
        let file = project_section_document(
            section,
            /*include_notice*/ false,
            &mut retained_body_rows,
            &mut body_overflow,
        );
        preview.files.push(file);
    }

    preview.truncated = body_overflow || files_truncated;
    preview
}

/// Plan a content diff against live file text when possible.
///
/// `read_path` receives each section path as written in the document and should
/// return UTF-8 file contents when the path is readable in the caller's
/// workspace. Sections that cannot be planned fall back to document rows with a
/// per-file notice and set [`EditPreviewKind::Planned::unverified`].
pub fn planned_edit(input: &str, mut read_path: impl FnMut(&str) -> Option<String>) -> EditPreview {
    let sections = match parse_hashline(input) {
        Ok(sections) => sections,
        // Incomplete / malformed documents stay on the document projection, but
        // approval still warns because nothing was verified against disk.
        Err(_) => {
            let mut preview = proposed_edit(input);
            mark_planned_unverified(&mut preview);
            return preview;
        }
    };

    let mut preview = EditPreview {
        kind: EditPreviewKind::Planned { unverified: false },
        ..EditPreview::default()
    };
    let mut retained_body_rows = 0usize;
    let mut body_overflow = false;
    let mut files_truncated = false;
    let mut unverified = false;

    for section in sections {
        if preview.files.len() >= MAX_PROPOSED_DIFF_FILES {
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
                    unverified = true;
                    project_section_document(
                        Section {
                            path,
                            tag: section.tag,
                            ops: section.ops,
                        },
                        /*include_notice*/ true,
                        &mut retained_body_rows,
                        &mut body_overflow,
                    )
                }
            },
            None => {
                unverified = true;
                project_section_document(
                    Section {
                        path,
                        tag: section.tag,
                        ops: section.ops,
                    },
                    /*include_notice*/ true,
                    &mut retained_body_rows,
                    &mut body_overflow,
                )
            }
        };
        preview.files.push(file);
    }

    preview.truncated = body_overflow || files_truncated;
    preview.kind = EditPreviewKind::Planned { unverified };
    preview
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
    include_notice: bool,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) -> DiffCardFile {
    let mut added_lines = 0u64;
    let mut removed_lines = 0u64;
    let mut rows = Vec::new();

    if include_notice {
        push_row(
            &mut rows,
            DiffRow::new(DiffRowKind::Meta, None, EDIT_DOCUMENT_ONLY_NOTICE),
            retained_body_rows,
            body_overflow,
        );
    }

    for op in &section.ops {
        project_op_document(
            op,
            &mut added_lines,
            &mut removed_lines,
            &mut rows,
            retained_body_rows,
            body_overflow,
        );
    }

    DiffCardFile {
        path: section.path,
        source_path: None,
        // Pure CUT is still in-file content change, never path deletion.
        change: DiffCardChange::Content,
        stats: Some((added_lines, removed_lines))
            .filter(|(added, removed)| *added > 0 || *removed > 0),
        rows,
    }
}

fn project_section_planned(
    path: String,
    original: &str,
    updated: &str,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) -> DiffCardFile {
    let diff = unified_diff(original, updated, &path, /*created*/ false);
    let parsed = parse_unified_diff(&diff);
    let file = parsed
        .into_iter()
        .next()
        .map(DiffCardFile::from)
        .unwrap_or_else(|| DiffCardFile {
            path: path.clone(),
            source_path: None,
            change: DiffCardChange::Content,
            stats: None,
            rows: Vec::new(),
        });
    // Keep the caller's document path for display even if the diff header
    // normalized differently.
    let mut file = DiffCardFile { path, ..file };
    trim_rows_to_budget(&mut file, retained_body_rows, body_overflow);
    file
}

fn trim_rows_to_budget(
    file: &mut DiffCardFile,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    if *retained_body_rows >= MAX_PROPOSED_DIFF_ROWS {
        if !file.rows.is_empty() {
            *body_overflow = true;
            file.rows.clear();
        }
        return;
    }
    let room = MAX_PROPOSED_DIFF_ROWS - *retained_body_rows;
    if file.rows.len() > room {
        file.rows.truncate(room);
        *body_overflow = true;
    }
    *retained_body_rows = retained_body_rows.saturating_add(file.rows.len());
}

fn project_op_document(
    op: &Op,
    added_lines: &mut u64,
    removed_lines: &mut u64,
    rows: &mut Vec<DiffRow>,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    match op {
        Op::Replace { start, end, body } => {
            *removed_lines += (*end - *start + 1) as u64;
            *added_lines += body.len() as u64;
            push_summary_and_body(
                rows,
                format_put_locator(*start, *end),
                body,
                retained_body_rows,
                body_overflow,
            );
        }
        Op::Delete { start, end } => {
            *removed_lines += (*end - *start + 1) as u64;
            push_row(
                rows,
                DiffRow::new(DiffRowKind::Meta, None, format_cut_locator(*start, *end)),
                retained_body_rows,
                body_overflow,
            );
        }
        Op::InsertBefore { line, body } => {
            *added_lines += body.len() as u64;
            push_summary_and_body(
                rows,
                format!("PUT <{line}"),
                body,
                retained_body_rows,
                body_overflow,
            );
        }
        Op::InsertAfter { line, body } => {
            *added_lines += body.len() as u64;
            let summary = match line {
                Some(line) => format!("PUT >{line}"),
                None => "PUT >$".into(),
            };
            push_summary_and_body(rows, summary, body, retained_body_rows, body_overflow);
        }
    }
}

fn push_summary_and_body(
    rows: &mut Vec<DiffRow>,
    summary: String,
    body: &[String],
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    push_row(
        rows,
        DiffRow::new(DiffRowKind::Meta, None, summary),
        retained_body_rows,
        body_overflow,
    );
    for text in body {
        push_row(
            rows,
            DiffRow::new(DiffRowKind::Added, None, text.clone()),
            retained_body_rows,
            body_overflow,
        );
    }
}

fn push_row(
    rows: &mut Vec<DiffRow>,
    row: DiffRow,
    retained_body_rows: &mut usize,
    body_overflow: &mut bool,
) {
    if *retained_body_rows >= MAX_PROPOSED_DIFF_ROWS {
        *body_overflow = true;
        return;
    }
    rows.push(row);
    *retained_body_rows += 1;
}

fn mark_planned_unverified(preview: &mut EditPreview) {
    preview.kind = EditPreviewKind::Planned { unverified: true };
    for file in &mut preview.files {
        let already = file.rows.first().is_some_and(|row| {
            row.kind == DiffRowKind::Meta && row.text == EDIT_DOCUMENT_ONLY_NOTICE
        });
        if !already {
            file.rows.insert(
                0,
                DiffRow::new(DiffRowKind::Meta, None, EDIT_DOCUMENT_ONLY_NOTICE),
            );
        }
    }
}

#[cfg(test)]
#[path = "proposed_tests.rs"]
mod tests;
