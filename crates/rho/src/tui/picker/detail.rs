//! Picker detail content: plain text or a structured sheet.
//!
//! Features describe what an item's detail says; this module decides how it
//! looks. A sheet gives the detail pane visual hierarchy (a bold title with a
//! tag, aligned label/value facts, section headings, muted notes) so the most
//! important facts fit above the fold instead of one dim wall of text.

use std::borrow::Cow;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{rows::picker_badge_style, PickerBadge};
use crate::tui::{
    render::{
        clip_line, display_width, truncate_keep_end, truncate_one_line, wrap_line_at_whitespace,
        wrap_text_lines,
    },
    theme::Theme,
};

/// Widest label column a field block reserves before values start.
const MAX_FIELD_LABEL_WIDTH: usize = 14;
const FIELD_GAP: usize = 2;
const TITLE_TAG_GAP: usize = 2;

/// Detail pane content for one picker item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PickerDetail {
    /// Plain wrapped text, drawn dim.
    Text(String),
    /// Structured blocks drawn with hierarchy.
    Sheet(DetailSheet),
}

impl From<String> for PickerDetail {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for PickerDetail {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<DetailSheet> for PickerDetail {
    fn from(sheet: DetailSheet) -> Self {
        Self::Sheet(sheet)
    }
}

impl PickerDetail {
    /// Unstyled text for search haystacks and one-line inline previews.
    pub(in crate::tui) fn plain_text(&self) -> Cow<'_, str> {
        match self {
            Self::Text(text) => Cow::Borrowed(text),
            Self::Sheet(sheet) => Cow::Owned(sheet.plain_text()),
        }
    }

    /// Cheap change fingerprint for the wrap cache.
    pub(super) fn content_len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Sheet(sheet) => sheet.blocks.len(),
        }
    }
}

/// Ordered detail blocks for one item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct DetailSheet {
    pub(in crate::tui) blocks: Vec<DetailBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum DetailBlock {
    /// Bold title with an optional tag aligned to the right edge.
    Title {
        text: String,
        tag: Option<PickerBadge>,
    },
    /// Wrapped body text at normal weight.
    Paragraph(String),
    /// Wrapped secondary text, drawn dim.
    Muted(String),
    /// Dim text clipped to `rows` wrapped rows, ending in an ellipsis when cut.
    Excerpt { text: String, rows: usize },
    /// Label/value rows sharing one value column.
    Fields(Vec<DetailField>),
    /// Accent heading with dim status text after it.
    Heading { label: String, status: String },
    /// Horizontal rule separating groups.
    Rule,
}

/// One label/value row. `note` trails the value in dim text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct DetailField {
    pub(in crate::tui) label: String,
    pub(in crate::tui) value: String,
    pub(in crate::tui) tone: DetailTone,
    pub(in crate::tui) note: Option<String>,
    pub(in crate::tui) overflow: DetailOverflow,
}

/// What a value wider than its column does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum DetailOverflow {
    /// Wrap at whitespace onto continuation rows.
    #[default]
    Wrap,
    /// Stay on one row and drop the start, for paths whose tail matters.
    KeepEnd,
}

impl DetailField {
    pub(in crate::tui) fn new(
        label: impl Into<String>,
        value: impl Into<String>,
        tone: DetailTone,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone,
            note: None,
            overflow: DetailOverflow::Wrap,
        }
    }

    pub(in crate::tui) fn keep_end(mut self) -> Self {
        self.overflow = DetailOverflow::KeepEnd;
        self
    }

    pub(in crate::tui) fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Emphasis for a field value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum DetailTone {
    /// Explicit, meaningful value.
    Normal,
    /// Default or inherited value that should recede.
    Muted,
    /// Missing or problematic value that needs attention.
    Warning,
}

impl DetailTone {
    fn style(self) -> Style {
        match self {
            Self::Normal => Theme::text(),
            Self::Muted => Theme::dim(),
            Self::Warning => Theme::warning(),
        }
    }
}

impl DetailSheet {
    fn plain_text(&self) -> String {
        let mut parts = Vec::new();
        for block in &self.blocks {
            match block {
                DetailBlock::Title { text, tag } => {
                    parts.push(text.clone());
                    if let Some(tag) = tag {
                        parts.push(tag.text.clone());
                    }
                }
                DetailBlock::Paragraph(text)
                | DetailBlock::Muted(text)
                | DetailBlock::Excerpt { text, .. } => parts.push(text.clone()),
                DetailBlock::Fields(fields) => {
                    for field in fields {
                        parts.push(format!("{} {}", field.label, field.value));
                        if let Some(note) = &field.note {
                            parts.push(note.clone());
                        }
                    }
                }
                DetailBlock::Heading { label, status } => parts.push(format!("{label} {status}")),
                DetailBlock::Rule => {}
            }
        }
        parts.join("\n")
    }
}

/// Styled, wrapped detail rows at `width`. Every row fits in `width` columns.
pub(in crate::tui) fn detail_lines(detail: &PickerDetail, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    match detail {
        PickerDetail::Text(text) => wrap_text_lines(text, width, Theme::dim()),
        PickerDetail::Sheet(sheet) => sheet_lines(sheet, width)
            .into_iter()
            .map(|line| clip_line(line, width))
            .collect(),
    }
}

fn sheet_lines(sheet: &DetailSheet, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in &sheet.blocks {
        match block {
            DetailBlock::Title { text, tag } => lines.push(title_line(text, tag.as_ref(), width)),
            DetailBlock::Paragraph(text) => {
                lines.extend(wrap_text_lines(text, width, Theme::text()))
            }
            DetailBlock::Muted(text) => lines.extend(wrap_text_lines(text, width, Theme::dim())),
            DetailBlock::Excerpt { text, rows } => lines.extend(excerpt_lines(text, *rows, width)),
            DetailBlock::Fields(fields) => lines.extend(field_lines(fields, width)),
            DetailBlock::Heading { label, status } => {
                lines.push(heading_line(label, status, width));
            }
            DetailBlock::Rule => {
                lines.push(Line::from(Span::styled("─".repeat(width), Theme::dim())))
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::raw(""));
    }
    lines
}

/// Whitespace-collapsed `text` wrapped to at most `rows` rows. A cut shows as
/// a trailing ellipsis on the last row.
fn excerpt_lines(text: &str, rows: usize, width: usize) -> Vec<Line<'static>> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let wrapped = wrap_line_at_whitespace(&flat, width);
    let cut = wrapped.len() > rows;
    wrapped
        .into_iter()
        .take(rows)
        .enumerate()
        .map(|(index, part)| {
            let part = part.trim_end();
            let text = if cut && index + 1 == rows {
                // Reserve one column so the ellipsis always fits.
                let kept = truncate_one_line(part, width.saturating_sub(1).max(1));
                format!("{}…", kept.trim_end_matches('…').trim_end())
            } else {
                part.to_owned()
            };
            Line::from(Span::styled(text, Theme::dim()))
        })
        .collect()
}

fn title_line(text: &str, tag: Option<&PickerBadge>, width: usize) -> Line<'static> {
    let title_style = Theme::text_strong();
    let Some(tag) = tag.filter(|_| width > TITLE_TAG_GAP + 2) else {
        return Line::from(Span::styled(truncate_one_line(text, width), title_style));
    };
    // The tag keeps its width until the title would drop below half the row.
    let tag_text = truncate_one_line(&tag.text, width / 2);
    let tag_width = display_width(&tag_text);
    let title_budget = width.saturating_sub(tag_width + TITLE_TAG_GAP);
    let title = truncate_one_line(text, title_budget);
    let pad = width
        .saturating_sub(display_width(&title))
        .saturating_sub(tag_width);
    Line::from(vec![
        Span::styled(title, title_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(tag_text, picker_badge_style(tag.tone)),
    ])
}

fn heading_line(label: &str, status: &str, width: usize) -> Line<'static> {
    let label = truncate_one_line(label, width);
    let used = display_width(&label);
    let mut spans = vec![Span::styled(label, Theme::brand())];
    let status_budget = width.saturating_sub(used + FIELD_GAP);
    if !status.is_empty() && status_budget > 0 {
        spans.push(Span::raw(" ".repeat(FIELD_GAP)));
        spans.push(Span::styled(
            truncate_one_line(status, status_budget),
            Theme::dim(),
        ));
    }
    Line::from(spans)
}

fn field_lines(fields: &[DetailField], width: usize) -> Vec<Line<'static>> {
    let label_width = fields
        .iter()
        .map(|field| display_width(&field.label))
        .max()
        .unwrap_or(0)
        .min(MAX_FIELD_LABEL_WIDTH)
        // Keep at least a few value columns on narrow panes.
        .min(width.saturating_sub(FIELD_GAP + 4));
    let value_at = label_width + FIELD_GAP;
    let value_width = width.saturating_sub(value_at).max(1);
    let continuation = " ".repeat(value_at.min(width));
    let mut lines = Vec::new();
    for field in fields {
        let label = if label_width == 0 {
            String::new()
        } else {
            let label = truncate_one_line(&field.label, label_width);
            let pad = value_at.saturating_sub(display_width(&label));
            format!("{label}{}", " ".repeat(pad))
        };
        let mut rows: Vec<Vec<Span<'static>>> = match field.overflow {
            DetailOverflow::Wrap => wrap_line_at_whitespace(&field.value, value_width)
                .into_iter()
                .map(|part| vec![Span::styled(part.to_owned(), field.tone.style())])
                .collect(),
            DetailOverflow::KeepEnd => vec![vec![Span::styled(
                truncate_keep_end(&field.value, value_width),
                field.tone.style(),
            )]],
        };
        if rows.is_empty() {
            rows.push(Vec::new());
        }
        if let Some(note) = field.note.as_deref().filter(|note| !note.is_empty()) {
            // Inline when the note fits after a one-row value; otherwise it
            // wraps on its own rows under the value column.
            let inline = rows.len() == 1
                && display_width(&field.value) + FIELD_GAP + display_width(note) <= value_width;
            if inline {
                rows[0].push(Span::raw(" ".repeat(FIELD_GAP)));
                rows[0].push(Span::styled(note.to_owned(), Theme::dim()));
            } else {
                rows.extend(
                    wrap_line_at_whitespace(note, value_width)
                        .into_iter()
                        .map(|part| vec![Span::styled(part.to_owned(), Theme::dim())]),
                );
            }
        }
        for (index, row) in rows.into_iter().enumerate() {
            let prefix = if index == 0 {
                Span::styled(label.clone(), Theme::dim())
            } else {
                Span::raw(continuation.clone())
            };
            let mut spans = vec![prefix];
            spans.extend(row);
            lines.push(Line::from(spans));
        }
    }
    lines
}

#[cfg(test)]
#[path = "detail_tests.rs"]
mod tests;
