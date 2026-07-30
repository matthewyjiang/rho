//! Best-effort projection of a streamed patch into display-ready diff rows.

use crate::tool_card::{DiffRow, DiffRowKind};

use super::parser::{
    ADD_FILE_MARKER, BEGIN_PATCH_MARKER, DELETE_FILE_MARKER, END_PATCH_MARKER, EOF_MARKER,
    MOVE_TO_MARKER, UPDATE_FILE_MARKER,
};

/// Maximum rendered rows for a proposed card, including multi-file headings and
/// one card-level truncation footer.
const MAX_PROPOSED_DIFF_ROWS: usize = 1_000;
/// Maximum file operations retained for a proposed card.
const MAX_PROPOSED_DIFF_FILES: usize = 100;

/// A best-effort proposed diff grouped by file operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProposedDiff {
    pub files: Vec<ProposedDiffFile>,
    /// True when file ops or body rows were dropped to stay within the card budget.
    pub truncated: bool,
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
/// end marker, and never assigns line numbers. Truncation is reported on
/// [`ProposedDiff::truncated`]; callers append a card-level footer when set.
pub fn proposed_diff_lenient(input: &str, trailing_line: ProposedDiffTrailingLine) -> ProposedDiff {
    let mut proposed = ProposedDiff::default();
    let mut current = None;
    let mut in_patch = false;
    let mut retained_body_rows = 0usize;
    let mut body_overflow = false;
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
        // One leading space is the update context prefix, so keep it. Zero or
        // two-plus spaces still allow section-marker detection.
        let section_marker = section_marker_line(marker);
        if section_marker == END_PATCH_MARKER {
            finish_file(&mut proposed.files, &mut current);
            break;
        }
        if let Some(path) = nonempty_marker_value(section_marker, ADD_FILE_MARKER) {
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
        if let Some(path) = nonempty_marker_value(section_marker, DELETE_FILE_MARKER) {
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
        if let Some(path) = nonempty_marker_value(section_marker, UPDATE_FILE_MARKER) {
            finish_file(&mut proposed.files, &mut current);
            move_allowed = start_file(
                &mut proposed.files,
                &mut current,
                ProposedDiffFile::update(path),
                &mut files_truncated,
            );
            continue;
        }
        if let Some(destination) = nonempty_marker_value(section_marker, MOVE_TO_MARKER) {
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
            // Soft-cap retained body rows during the scan so huge patches do not
            // allocate unbounded row vectors. finalize() then applies the single
            // card budget once headings are known.
            file.record_row(kind, text, &mut retained_body_rows, &mut body_overflow);
        }
    }

    finish_file(&mut proposed.files, &mut current);
    proposed.finalize(body_overflow || files_truncated);
    proposed
}

fn nonempty_marker_value<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.strip_prefix(marker).filter(|value| !value.is_empty())
}

/// Normalize a line for section-marker matching.
///
/// A single leading space is the update-hunk context prefix, so those lines stay
/// intact and cannot match `*** …` markers. Unindented markers and markers with
/// two or more leading spaces remain detectable.
fn section_marker_line(marker: &str) -> &str {
    let stripped = marker.trim_start();
    let leading = marker.len() - stripped.len();
    if leading == 1 {
        marker
    } else {
        stripped
    }
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

impl ProposedDiff {
    /// Apply one card-level row budget shared by multi-file headings, body rows,
    /// and an optional truncation footer reserved by the presenter.
    fn finalize(&mut self, already_truncated: bool) {
        let heading_rows = if self.files.len() > 1 {
            self.files.len()
        } else {
            0
        };
        let body_rows = self.files.iter().map(|file| file.rows.len()).sum::<usize>();
        let fits_without_footer = heading_rows + body_rows <= MAX_PROPOSED_DIFF_ROWS;
        let truncated = already_truncated || !fits_without_footer;
        // Reserve one slot for the card-level footer whenever truncation is
        // reported and headings leave room for it.
        let footer_rows = usize::from(truncated && heading_rows < MAX_PROPOSED_DIFF_ROWS);
        let body_budget = MAX_PROPOSED_DIFF_ROWS
            .saturating_sub(heading_rows)
            .saturating_sub(footer_rows);
        let mut remaining = body_budget;
        for file in &mut self.files {
            if file.rows.len() > remaining {
                file.rows.truncate(remaining);
                remaining = 0;
            } else {
                remaining -= file.rows.len();
            }
        }
        self.truncated = truncated;
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

    fn record_row(
        &mut self,
        kind: DiffRowKind,
        text: &str,
        retained_body_rows: &mut usize,
        body_overflow: &mut bool,
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
        // Retain up to the full card budget of body rows; finalize may shrink
        // further once multi-file headings claim part of the budget.
        if *retained_body_rows < MAX_PROPOSED_DIFF_ROWS {
            self.rows.push(DiffRow::new(kind, None, text));
            *retained_body_rows += 1;
        } else {
            *body_overflow = true;
        }
    }
}
