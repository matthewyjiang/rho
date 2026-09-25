//! One file's git patch as detail-pane rows for the `/diff` viewer.
//!
//! [`patch_rows`] turns `git diff` output for a single file into numbered
//! [`DiffRow`]s; [`diff_pane_lines`] paints them with the same row painter as
//! tool cards, without the card's tree indent.

use ratatui::text::{Line, Span};
use rho_tools::tool_card::{DiffRow, DiffRowKind};

use super::{
    render::{truncate_keep_end, wrap_line_hard},
    theme::Theme,
    tool_diff::{gutter_width, push_diff_content_row, DiffRowFrame, DiffSyntax},
};

/// Rows for one file's patch.
///
/// Hunk bodies are consumed by the line counts in their `@@` header, so a
/// removed `-- comment` or added `++x` line is never mistaken for a file
/// header. Each hunk opens with a [`DiffRowKind::Skip`] row carrying the `@@`
/// line (git's function context is useful when jumping between hunks).
/// Header facts worth seeing (new/deleted mode, renames, binary notices) become
/// [`DiffRowKind::Meta`] rows; `diff --git`, `index`, and `---`/`+++` are
/// dropped because the pane heading already names the file.
pub(super) fn patch_rows(patch: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut old_line = 0u32;
    let mut new_line = 0u32;
    let mut old_left = 0u32;
    let mut new_left = 0u32;
    for line in patch.lines() {
        if old_left > 0 || new_left > 0 {
            let (kind, number) = match line.as_bytes().first() {
                Some(b'+') => {
                    new_left = new_left.saturating_sub(1);
                    new_line += 1;
                    (DiffRowKind::Added, new_line - 1)
                }
                Some(b'-') => {
                    old_left = old_left.saturating_sub(1);
                    old_line += 1;
                    (DiffRowKind::Removed, old_line - 1)
                }
                Some(b'\\') => {
                    rows.push(DiffRow::new(DiffRowKind::Meta, None, line));
                    continue;
                }
                // Context, including an empty line from tools that strip the
                // leading space.
                Some(_) | None => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    old_line += 1;
                    new_line += 1;
                    (DiffRowKind::Context, new_line - 1)
                }
            };
            rows.push(DiffRow::new(
                kind,
                Some(number),
                line.get(1..).unwrap_or_default(),
            ));
            continue;
        }
        if let Some(hunk) = HunkHeader::parse(line) {
            (old_line, old_left, new_line, new_left) =
                (hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len);
            rows.push(DiffRow::new(DiffRowKind::Skip, None, line));
            continue;
        }
        if is_hidden_header(line) {
            continue;
        }
        rows.push(DiffRow::new(DiffRowKind::Meta, None, line));
    }
    rows
}

/// Styled pane rows: the heading, then the patch rows at `width`.
pub(super) fn diff_pane_lines(
    heading: &str,
    path: &str,
    rows: &[DiffRow],
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            truncate_keep_end(heading, width),
            Theme::tool_path(),
        )),
        Line::raw(""),
    ];
    if rows.is_empty() {
        lines.push(Line::from(Span::styled("no textual changes", Theme::dim())));
        return lines;
    }
    let frame = DiffRowFrame {
        indent: "",
        gutter: gutter_width(rows),
        width,
    };
    let mut syntax = DiffSyntax::for_file(path);
    for row in rows {
        let style = match row.kind {
            DiffRowKind::Added | DiffRowKind::Removed | DiffRowKind::Context => {
                push_diff_content_row(&mut lines, row, frame, &mut syntax);
                continue;
            }
            DiffRowKind::Skip => {
                // A hunk gap desyncs multi-line token state; restart parsing.
                let _ = syntax.paint_row(row);
                Theme::accent()
            }
            DiffRowKind::Meta | DiffRowKind::File => Theme::dim(),
        };
        lines.extend(
            wrap_line_hard(&row.text, width)
                .into_iter()
                .map(|chunk| Line::from(Span::styled(chunk.to_string(), style))),
        );
    }
    lines
}

/// `+added -removed` counts for rows parsed by [`patch_rows`].
pub(super) fn row_stats(rows: &[DiffRow]) -> (u64, u64) {
    rows.iter()
        .fold((0, 0), |(added, removed), row| match row.kind {
            DiffRowKind::Added => (added + 1, removed),
            DiffRowKind::Removed => (added, removed + 1),
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip | DiffRowKind::Meta => {
                (added, removed)
            }
        })
}

/// Ranges from `@@ -a[,b] +c[,d] @@`. An omitted length means one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HunkHeader {
    old_start: u32,
    old_len: u32,
    new_start: u32,
    new_len: u32,
}

impl HunkHeader {
    fn parse(line: &str) -> Option<Self> {
        let (ranges, _) = line.strip_prefix("@@ ")?.split_once(" @@")?;
        let (old, new) = ranges.split_once(' ')?;
        let range = |range: &str, sign: char| -> Option<(u32, u32)> {
            let range = range.strip_prefix(sign)?;
            match range.split_once(',') {
                Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
                None => Some((range.parse().ok()?, 1)),
            }
        };
        let (old_start, old_len) = range(old, '-')?;
        let (new_start, new_len) = range(new, '+')?;
        Some(Self {
            old_start,
            old_len,
            new_start,
            new_len,
        })
    }
}

/// Header lines the pane heading already covers.
fn is_hidden_header(line: &str) -> bool {
    line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
}

#[cfg(test)]
#[path = "diff_pane_tests.rs"]
mod tests;
