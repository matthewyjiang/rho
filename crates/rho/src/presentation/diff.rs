//! Unified diffs parsed into the compact rows of a tool card's diff body.

use rho_protocol::display::{DiffRow, DiffRowKind};

/// One file section from a unified diff parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedDiffFile {
    pub(crate) path: String,
    pub(crate) added: u64,
    pub(crate) removed: u64,
    pub(crate) rows: Vec<DiffRow>,
}

/// Kind of file change represented by a [`DiffCardFile`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DiffCardChange {
    /// Add or update content, including moves that keep body rows.
    #[default]
    Content,
    /// Delete a file. Body rows are usually empty during a streamed preview.
    Delete,
}

/// One file section ready for a FileDiff tool card body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffCardFile {
    pub(crate) path: String,
    /// Prior path when this section is a move/rename.
    pub(crate) source_path: Option<String>,
    pub(crate) change: DiffCardChange,
    /// Present when the section has known addition/removal counts.
    pub(crate) stats: Option<(u64, u64)>,
    pub(crate) rows: Vec<DiffRow>,
}

impl DiffCardFile {
    /// Path shown in headers and multi-file body headings.
    pub(crate) fn display_path(&self) -> String {
        match &self.source_path {
            Some(source) if source != &self.path => format!("{source} → {}", self.path),
            _ => self.path.clone(),
        }
    }
}

impl From<ParsedDiffFile> for DiffCardFile {
    fn from(file: ParsedDiffFile) -> Self {
        Self {
            path: file.path,
            source_path: None,
            change: DiffCardChange::Content,
            stats: Some((file.added, file.removed)),
            rows: file.rows,
        }
    }
}

/// Parse a unified diff once; callers derive stats, body rows, and path counts.
pub(crate) fn parse_unified_diff(diff: &str) -> Vec<ParsedDiffFile> {
    let mut files = Vec::new();
    let mut current: Option<OpenDiffFile> = None;
    let mut pending_old_path: Option<String> = None;
    let mut in_hunk = false;

    for line in diff.lines() {
        if should_exit_hunk(line) {
            in_hunk = false;
        }

        if let Some(path) = minus_file_path(line) {
            pending_old_path = Some(path);
            continue;
        }

        if let Some(new_path) = plus_file_path(line) {
            if let Some(file) = current.take() {
                files.push(file.finish());
            }
            let old_path = pending_old_path.take();
            current = Some(OpenDiffFile {
                path: display_diff_path(old_path.as_deref(), &new_path),
                added: 0,
                removed: 0,
                rows: Vec::new(),
                next_old: 1,
                next_new: 1,
                seen_hunk: false,
            });
            continue;
        }

        if let Some((old_start, new_start)) = hunk_starts(line) {
            in_hunk = true;
            if let Some(file) = current.as_mut() {
                // A second hunk means skipped lines; mark the gap so the row
                // numbers do not look like a contiguous run.
                if file.seen_hunk {
                    file.rows.push(DiffRow::new(DiffRowKind::Skip, None, "⋯"));
                }
                file.seen_hunk = true;
                file.next_old = old_start;
                file.next_new = new_start;
            }
            continue;
        }

        if !in_hunk || line.is_empty() || line.starts_with('\\') {
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };
        let Some(marker) = line.as_bytes().first().copied() else {
            continue;
        };
        let content = line.get(1..).unwrap_or_default().to_string();
        match marker {
            b'+' => {
                file.added += 1;
                file.rows.push(DiffRow::new(
                    DiffRowKind::Added,
                    Some(file.next_new),
                    content,
                ));
                file.next_new += 1;
            }
            b'-' => {
                file.removed += 1;
                file.rows.push(DiffRow::new(
                    DiffRowKind::Removed,
                    Some(file.next_old),
                    content,
                ));
                file.next_old += 1;
            }
            b' ' => {
                file.rows.push(DiffRow::new(
                    DiffRowKind::Context,
                    Some(file.next_new),
                    content,
                ));
                file.next_old += 1;
                file.next_new += 1;
            }
            _ => {}
        }
    }

    if let Some(file) = current {
        files.push(file.finish());
    }
    files
}

/// Collapse a unified diff to numbered add/remove/context rows for the card body.
pub(crate) fn compact_diff_rows(diff: &str, include_file_headers: bool) -> Vec<DiffRow> {
    compact_diff_rows_from_files(&parse_unified_diff(diff), include_file_headers)
}

/// Build compact body rows from an already-parsed unified diff.
fn compact_diff_rows_from_files(
    files: &[ParsedDiffFile],
    include_file_headers: bool,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for file in files {
        if include_file_headers {
            rows.push(DiffRow::file_header(file.path.clone(), None));
        }
        rows.extend(file.rows.iter().cloned());
    }
    rows
}

/// Build compact body rows from card-facing file sections.
///
/// When `include_file_headers` is true, each section starts with a File row.
/// Content sections keep known `+N -M` counts on the row's structured `stats`
/// field so multi-file cards can drop separate DiffStat facts without losing
/// identity or counts (and without encoding stats into path text).
pub(crate) fn compact_diff_rows_from_card_files(
    files: &[DiffCardFile],
    include_file_headers: bool,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for file in files {
        if include_file_headers {
            let stats = match (file.change, file.stats) {
                (DiffCardChange::Content, stats) => stats,
                _ => None,
            };
            rows.push(DiffRow::file_header(file.display_path(), stats));
        }
        rows.extend(file.rows.iter().cloned());
    }
    rows
}

#[derive(Clone, Debug)]
struct OpenDiffFile {
    path: String,
    added: u64,
    removed: u64,
    rows: Vec<DiffRow>,
    next_old: u32,
    next_new: u32,
    seen_hunk: bool,
}

impl OpenDiffFile {
    fn finish(self) -> ParsedDiffFile {
        ParsedDiffFile {
            path: self.path,
            added: self.added,
            removed: self.removed,
            rows: self.rows,
        }
    }
}

/// Prefer the surviving path: new path unless the file was deleted.
fn display_diff_path(old_path: Option<&str>, new_path: &str) -> String {
    if new_path == "/dev/null" {
        old_path.unwrap_or(new_path).to_string()
    } else {
        new_path.to_string()
    }
}

/// Old and new starting line numbers from an `@@ -a,b +c,d @@` header.
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let body = line.strip_prefix("@@ ")?;
    let (ranges, _) = body.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let start = |range: &str, sign: char| -> Option<u32> {
        range
            .strip_prefix(sign)?
            .split(',')
            .next()?
            .parse::<u32>()
            .ok()
    };
    Some((start(old, '-')?, start(new, '+')?))
}

fn strip_diff_path(line: &str, prefixes: &[&str]) -> Option<String> {
    for prefix in prefixes {
        if let Some(path) = line.strip_prefix(prefix) {
            let path = path.split('\t').next().unwrap_or(path).trim();
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    None
}

fn minus_file_path(line: &str) -> Option<String> {
    if line.starts_with("--- /dev/null") {
        return Some("/dev/null".into());
    }
    strip_diff_path(line, &["--- a/", "--- b/", "--- "])
}

fn plus_file_path(line: &str) -> Option<String> {
    if line.starts_with("+++ /dev/null") {
        return Some("/dev/null".into());
    }
    strip_diff_path(line, &["+++ b/", "+++ a/", "+++ "])
}

/// Leave hunk mode at file boundaries so multi-file diffs need no blank separator.
fn should_exit_hunk(line: &str) -> bool {
    line.is_empty() || line.starts_with("diff --git") || is_file_header_line(line)
}

fn is_file_header_line(line: &str) -> bool {
    line.starts_with("--- ") || line.starts_with("+++ ")
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
