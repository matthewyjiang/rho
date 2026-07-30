//! Best-effort projection of a streamed patch into display-ready diff rows.

use crate::tool_card::{DiffRow, DiffRowKind};

use super::parser::{
    ADD_FILE_MARKER, BEGIN_PATCH_MARKER, DELETE_FILE_MARKER, END_PATCH_MARKER, EOF_MARKER,
    MOVE_TO_MARKER, UPDATE_FILE_MARKER,
};

/// Maximum proposed body rows, including file headings and one truncation row.
const MAX_PROPOSED_DIFF_ROWS: usize = 1_000;
/// Maximum file operations retained for a proposed card.
const MAX_PROPOSED_DIFF_FILES: usize = 100;

/// A best-effort proposed diff grouped by file operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProposedDiff {
    pub files: Vec<ProposedDiffFile>,
}

/// The patch operation represented by a proposed diff file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposedDiffOperation {
    Add,
    Update,
    Delete,
}

/// One file operation and the rows available in the patch stream so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedDiffFile {
    pub operation: ProposedDiffOperation,
    /// Path a presenter should use as the file heading.
    pub display_path: String,
    /// Existing path for updates, moves, and deletes.
    pub source_path: Option<String>,
    /// New path for adds and moves.
    pub destination_path: Option<String>,
    pub rows: Vec<DiffRow>,
    /// Known added row count. This is zero for a delete operation.
    pub added_lines: Option<u64>,
    /// Known removed row count. A delete operation has no body, so its count is unknown.
    pub removed_lines: Option<u64>,
}

/// Whether to project a final input line that has no newline terminator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProposedDiffTrailingLine {
    /// Ignore the trailing line. This is safe for an input stream still in progress.
    #[default]
    CompleteLinesOnly,
    /// Treat the trailing line as complete. Use this for a complete tool call.
    Include,
}

/// Project complete patch lines into display-ready rows without reading any files.
///
/// This parser is intentionally lenient so it can show an in-progress patch. It
/// ignores markers and invalid lines, accepts open file sections and a missing
/// end marker, and never assigns line numbers.
pub fn proposed_diff_lenient(input: &str, trailing_line: ProposedDiffTrailingLine) -> ProposedDiff {
    let mut proposed = ProposedDiff::default();
    let mut current = None;
    let mut in_patch = false;
    let mut retained_rows = 0usize;
    let mut body_truncated = false;
    let mut files_truncated = false;
    let mut move_allowed = false;

    for segment in input.split_inclusive('\n') {
        let is_complete = segment.ends_with('\n');
        if !is_complete && trailing_line == ProposedDiffTrailingLine::CompleteLinesOnly {
            break;
        }
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim();

        if !in_patch {
            if trimmed == BEGIN_PATCH_MARKER {
                in_patch = true;
            }
            continue;
        }
        let marker = line.trim_end();
        let outer_marker = marker.trim_start();
        if outer_marker == END_PATCH_MARKER {
            finish_file(&mut proposed.files, &mut current);
            break;
        }
        if let Some(path) = nonempty_marker_value(outer_marker, ADD_FILE_MARKER) {
            finish_file(&mut proposed.files, &mut current);
            start_file(
                &mut proposed.files,
                &mut current,
                ProposedDiffFile::add(path),
                &mut files_truncated,
            );
            move_allowed = false;
            continue;
        }
        if let Some(path) = nonempty_marker_value(outer_marker, DELETE_FILE_MARKER) {
            finish_file(&mut proposed.files, &mut current);
            start_file(
                &mut proposed.files,
                &mut current,
                ProposedDiffFile::delete(path),
                &mut files_truncated,
            );
            move_allowed = false;
            continue;
        }
        if let Some(path) = nonempty_marker_value(outer_marker, UPDATE_FILE_MARKER) {
            finish_file(&mut proposed.files, &mut current);
            move_allowed = start_file(
                &mut proposed.files,
                &mut current,
                ProposedDiffFile::update(path),
                &mut files_truncated,
            );
            continue;
        }
        if let Some(destination) = nonempty_marker_value(marker, MOVE_TO_MARKER) {
            if move_allowed {
                if let Some(
                    file @ ProposedDiffFile {
                        operation: ProposedDiffOperation::Update,
                        ..
                    },
                ) = current.as_mut()
                {
                    file.display_path = destination.to_string();
                    file.destination_path = Some(destination.to_string());
                }
            }
            move_allowed = false;
            continue;
        }
        move_allowed = false;
        if marker.starts_with("@@") || marker == EOF_MARKER || marker.starts_with("***") {
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };
        let row = match file.operation {
            ProposedDiffOperation::Add => line
                .strip_prefix('+')
                .map(|text| (DiffRowKind::Added, text)),
            ProposedDiffOperation::Update => {
                if let Some(text) = line.strip_prefix('+') {
                    Some((DiffRowKind::Added, text))
                } else if let Some(text) = line.strip_prefix('-') {
                    Some((DiffRowKind::Removed, text))
                } else if line.is_empty() {
                    Some((DiffRowKind::Context, ""))
                } else {
                    line.strip_prefix(' ')
                        .map(|text| (DiffRowKind::Context, text))
                }
            }
            ProposedDiffOperation::Delete => None,
        };
        if let Some((kind, text)) = row {
            file.push_row(kind, text, &mut retained_rows, &mut body_truncated);
        }
    }

    finish_file(&mut proposed.files, &mut current);
    bound_rendered_rows(&mut proposed.files, body_truncated || files_truncated);
    proposed
}

fn nonempty_marker_value<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.strip_prefix(marker).filter(|value| !value.is_empty())
}

fn finish_file(files: &mut Vec<ProposedDiffFile>, current: &mut Option<ProposedDiffFile>) {
    if let Some(file) = current.take() {
        files.push(file);
    }
}

fn start_file(
    files: &mut [ProposedDiffFile],
    current: &mut Option<ProposedDiffFile>,
    file: ProposedDiffFile,
    files_truncated: &mut bool,
) -> bool {
    if files.len() < MAX_PROPOSED_DIFF_FILES {
        *current = Some(file);
        true
    } else {
        *files_truncated = true;
        false
    }
}

fn bound_rendered_rows(files: &mut [ProposedDiffFile], truncated: bool) {
    let heading_rows = if files.len() > 1 { files.len() } else { 0 };
    let current_rows = files.iter().map(|file| file.rows.len()).sum::<usize>();
    let content_budget = MAX_PROPOSED_DIFF_ROWS.saturating_sub(heading_rows);
    let needs_truncation = truncated || current_rows > content_budget;
    let retained_budget = content_budget.saturating_sub(usize::from(needs_truncation));
    let mut remaining = retained_budget;
    for file in files.iter_mut() {
        if file.rows.len() > remaining {
            file.rows.truncate(remaining);
            remaining = 0;
        } else {
            remaining -= file.rows.len();
        }
    }
    if !needs_truncation || content_budget == 0 {
        return;
    }
    if let Some(last) = files.last_mut() {
        last.rows
            .push(DiffRow::new(DiffRowKind::Skip, None, "⋯ more changes"));
    }
}

impl ProposedDiffFile {
    fn add(path: &str) -> Self {
        Self {
            operation: ProposedDiffOperation::Add,
            display_path: path.to_string(),
            source_path: None,
            destination_path: Some(path.to_string()),
            rows: Vec::new(),
            added_lines: Some(0),
            removed_lines: Some(0),
        }
    }

    fn update(path: &str) -> Self {
        Self {
            operation: ProposedDiffOperation::Update,
            display_path: path.to_string(),
            source_path: Some(path.to_string()),
            destination_path: None,
            rows: Vec::new(),
            added_lines: Some(0),
            removed_lines: Some(0),
        }
    }

    fn delete(path: &str) -> Self {
        Self {
            operation: ProposedDiffOperation::Delete,
            display_path: path.to_string(),
            source_path: Some(path.to_string()),
            destination_path: None,
            rows: Vec::new(),
            added_lines: Some(0),
            removed_lines: None,
        }
    }

    fn push_row(
        &mut self,
        kind: DiffRowKind,
        text: &str,
        retained_rows: &mut usize,
        body_truncated: &mut bool,
    ) {
        match kind {
            DiffRowKind::Added => {
                self.added_lines = self.added_lines.map(|count| count + 1);
            }
            DiffRowKind::Removed => {
                self.removed_lines = self.removed_lines.map(|count| count + 1);
            }
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip => {}
        }
        if *retained_rows < MAX_PROPOSED_DIFF_ROWS {
            self.rows.push(DiffRow::new(kind, None, text));
            *retained_rows += 1;
        } else {
            *body_truncated = true;
        }
    }
}
