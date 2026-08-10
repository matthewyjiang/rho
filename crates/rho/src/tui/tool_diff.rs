use rho_tools::tool_card::{DiffRow, DiffRowKind, ToolFamily, ToolHeader};

use super::syntax::{
    BlockHighlighter, HighlightSegment, MAX_TOOL_SYNTAX_LINES, MAX_TOOL_SYNTAX_LINE_BYTES,
};

/// Width of the line-number gutter for a diff body.
///
/// Zero when no row carries a number, so patch text without numbering (the
/// `/diff` command) renders without an empty column.
pub(super) fn gutter_width(rows: &[DiffRow]) -> usize {
    rows.iter()
        .filter_map(|row| row.line)
        .max()
        .map_or(0, |line| line.to_string().len())
}

pub(super) fn logical_lines(display_lines: &[String]) -> Vec<String> {
    display_lines
        .iter()
        .flat_map(|line| {
            let lines = line.lines().map(str::to_string).collect::<Vec<_>>();
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            }
        })
        .collect()
}

/// Single-file write/edit path from the card header when the body has no
/// [`DiffRowKind::File`] rows. Multi-file cards always emit File rows, so the
/// renderer prefers those and leaves this unused.
pub(super) fn single_file_path_from_header(
    family: ToolFamily,
    header: &ToolHeader,
) -> Option<&str> {
    if family != ToolFamily::FileDiff {
        return None;
    }
    match header {
        ToolHeader::Call {
            primary: Some(path),
            ..
        } if !path.is_empty() => Some(path.as_str()),
        _ => None,
    }
}

/// Tracks old/new highlighters for one file so context advances both sides and
/// add/remove only touch their own stream (multi-line tokens stay accurate).
pub(super) struct DiffSyntax {
    path: Option<String>,
    old: Option<BlockHighlighter>,
    new: Option<BlockHighlighter>,
    /// Content lines (add/remove/context source) already language-painted.
    highlighted_lines: usize,
}

impl DiffSyntax {
    pub(super) fn new(fallback_path: Option<&str>) -> Self {
        let mut syntax = Self {
            path: None,
            old: None,
            new: None,
            highlighted_lines: 0,
        };
        if let Some(path) = fallback_path {
            syntax.set_path(path);
        }
        syntax
    }

    /// Observe path/skip chrome and highlight content for one row.
    ///
    /// Returns role segments for add/remove/context source lines, or `None` for
    /// chrome, unknown languages, disabled highlight, or past the soft cap.
    pub(super) fn paint_row(&mut self, row: &DiffRow) -> Option<Vec<HighlightSegment>> {
        match row.kind {
            DiffRowKind::File => {
                self.set_path(&row.text);
                None
            }
            DiffRowKind::Skip => {
                // Hunk gap: missing lines would desync parse state, so restart.
                self.restart();
                None
            }
            DiffRowKind::Meta => {
                if let Some(path) = path_from_diff_header_line(&row.text) {
                    self.set_path(path);
                }
                None
            }
            DiffRowKind::Added => self.paint_content(/*side*/ Side::New, &row.text),
            DiffRowKind::Removed => self.paint_content(/*side*/ Side::Old, &row.text),
            DiffRowKind::Context => {
                if let Some(path) = path_from_diff_header_line(&row.text) {
                    self.set_path(path);
                    return None;
                }
                // Keep unified-diff headers and git chrome out of the parse
                // stream so language state only sees source lines.
                if is_diff_chrome(&row.text) {
                    return None;
                }
                if !self.should_paint_content_line(&row.text) {
                    // Long / over-budget lines skip syntect entirely. Restart so
                    // the next short line does not inherit a desynced stack.
                    if row.text.len() > MAX_TOOL_SYNTAX_LINE_BYTES {
                        self.restart();
                    }
                    return None;
                }
                // Advance old without segment alloc; styles come from new.
                if let Some(old) = self.old.as_mut() {
                    old.advance_line(&row.text);
                }
                self.paint_content(/*side*/ Side::New, &row.text)
            }
        }
    }

    fn should_paint_content(&self) -> bool {
        self.highlighted_lines < MAX_TOOL_SYNTAX_LINES
    }

    fn should_paint_content_line(&self, text: &str) -> bool {
        self.should_paint_content() && text.len() <= MAX_TOOL_SYNTAX_LINE_BYTES
    }

    fn paint_content(&mut self, side: Side, text: &str) -> Option<Vec<HighlightSegment>> {
        if !self.should_paint_content_line(text) {
            // Soft caps: plain row colors, no more syntect work this pass.
            // Over-long add/remove lines restart only their side so the other
            // stream keeps multi-line token state.
            if text.len() > MAX_TOOL_SYNTAX_LINE_BYTES {
                self.restart_side(side);
            }
            return None;
        }
        let segments = match side {
            Side::New => self.new.as_mut().map(|hl| hl.highlight_line(text)),
            Side::Old => self.old.as_mut().map(|hl| hl.highlight_line(text)),
        };
        if segments.is_some() {
            self.highlighted_lines += 1;
        }
        segments
    }

    fn set_path(&mut self, path: &str) {
        let path = normalize_diff_path(path);
        if path.is_empty() || path == "/dev/null" {
            return;
        }
        if self.path.as_deref() == Some(path) {
            return;
        }
        self.path = Some(path.to_string());
        self.old = BlockHighlighter::for_path(path);
        self.new = BlockHighlighter::for_path(path);
        self.highlighted_lines = 0;
    }

    fn restart(&mut self) {
        if let Some(path) = self.path.clone() {
            self.old = BlockHighlighter::for_path(&path);
            self.new = BlockHighlighter::for_path(&path);
        }
    }

    fn restart_side(&mut self, side: Side) {
        let Some(path) = self.path.clone() else {
            return;
        };
        match side {
            Side::Old => self.old = BlockHighlighter::for_path(&path),
            Side::New => self.new = BlockHighlighter::for_path(&path),
        }
    }
}

enum Side {
    Old,
    New,
}

/// Strip unified-diff path prefixes and rename arrows for language lookup.
fn normalize_diff_path(path: &str) -> &str {
    let path = path.trim();
    let path = path
        .rsplit_once(" → ")
        .map(|(_, dest)| dest.trim())
        .unwrap_or(path);
    let path = path
        .strip_prefix("b/")
        .or_else(|| path.strip_prefix("a/"))
        .unwrap_or(path);
    path.trim()
}

/// Best-effort path from a unified-diff header line (`+++`, `---`, `diff --git`).
fn path_from_diff_header_line(line: &str) -> Option<&str> {
    let line = line.trim_end();
    if let Some(rest) = line
        .strip_prefix("+++ ")
        .or_else(|| line.strip_prefix("--- "))
    {
        // Optional git prefixes: `+++ b/foo` or tab-separated timestamps.
        let path = rest.split('\t').next().unwrap_or(rest).trim();
        let path = normalize_diff_path(path);
        if path.is_empty() || path == "/dev/null" {
            return None;
        }
        return Some(path);
    }
    if let Some(rest) = line.strip_prefix("diff --git ") {
        // `diff --git a/old b/new` — prefer the new path.
        let mut parts = rest.split_whitespace();
        let _old = parts.next()?;
        let new = parts.next().unwrap_or(_old);
        let path = normalize_diff_path(new);
        if path.is_empty() || path == "/dev/null" {
            return None;
        }
        return Some(path);
    }
    None
}

/// Headers and git metadata that must not train the language highlighter.
/// Path-bearing headers are handled before this via [`path_from_diff_header_line`].
fn is_diff_chrome(text: &str) -> bool {
    text.starts_with("@@")
        || text.starts_with("index ")
        || text.starts_with("new file mode")
        || text.starts_with("deleted file mode")
        || text.starts_with("old mode")
        || text.starts_with("new mode")
        || text.starts_with("similarity index")
        || text.starts_with("rename from")
        || text.starts_with("rename to")
        || text.starts_with("Binary files")
        || text.starts_with("\\ ")
}

#[cfg(test)]
#[path = "tool_diff_tests.rs"]
mod tests;
