use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use unicode_width::UnicodeWidthStr;

use super::{
    render::{truncate_one_line, wrap_line_at_whitespace},
    theme::Theme,
};

/// The narrowest label column, after the indent. A section whose widest
/// label leaves no space before this column widens it for all its fields.
const FIELD_LABEL_WIDTH: usize = 14;
const SIDE_BY_SIDE_MIN_WIDTH: usize = 32;

/// Labelled fields and notes laid out as sections: each section's values
/// start in one column, set by its widest label once the section closes.
pub(super) struct CommandBlock {
    lines: Vec<Line<'static>>,
    /// The section being built, laid out when the next section or header
    /// starts, or at [`Self::finish`].
    open: Vec<Row>,
    style: Style,
    width: usize,
}

/// A row of the open section.
enum Row {
    Field { label: String, value: String },
    Note(Vec<Line<'static>>),
}

impl CommandBlock {
    pub(super) fn new(width: usize) -> Self {
        Self {
            lines: Vec::new(),
            open: Vec::new(),
            style: Theme::command_block(),
            width,
        }
    }

    /// Same layout as [`Self::new`], without the transcript card background.
    /// Overlay panels paint their own surface, so a block fill would cover it.
    pub(super) fn on_surface(width: usize) -> Self {
        Self {
            lines: Vec::new(),
            open: Vec::new(),
            style: Theme::text(),
            width,
        }
    }

    pub(super) fn push_header(&mut self, title: &str, detail: &str) {
        self.close_section();
        if self.width == 0 {
            self.lines.push(Line::from(Span::styled("", self.style)));
            return;
        }
        let title = truncate_one_line(title, self.width);
        let mut spans = vec![Span::styled(
            title.clone(),
            self.style.add_modifier(Modifier::BOLD),
        )];
        let remaining = self.width.saturating_sub(Line::from(title).width());
        if remaining > 2 {
            spans.push(Span::styled("  ", self.style));
            spans.push(Span::styled(
                truncate_one_line(detail, remaining - 2),
                self.style.add_modifier(Modifier::DIM),
            ));
        }
        self.lines.push(Line::from(spans));
    }

    pub(super) fn push_section(&mut self, title: &str) {
        self.close_section();
        self.lines.push(Line::from(Span::styled("", self.style)));
        self.lines.push(Line::from(Span::styled(
            truncate_one_line(title, self.width),
            self.style.add_modifier(Modifier::BOLD),
        )));
    }

    pub(super) fn push_field(&mut self, label: &str, value: &str) {
        self.open.push(Row::Field {
            label: label.to_owned(),
            value: value.to_owned(),
        });
    }

    /// Lays out the open section's rows, its fields' values in one column.
    fn close_section(&mut self) {
        let rows = std::mem::take(&mut self.open);
        let widest = rows
            .iter()
            .filter_map(|row| match row {
                Row::Field { label, .. } => Some(label.width()),
                Row::Note(_) => None,
            })
            .max()
            .unwrap_or_default();
        let label_width = FIELD_LABEL_WIDTH
            .max(widest + 1)
            .min(self.width.saturating_sub(1));
        for row in rows {
            match row {
                Row::Field { label, value } => self.field_lines(&label, &value, label_width),
                Row::Note(lines) => self.lines.extend(lines),
            }
        }
    }

    fn field_lines(&mut self, label: &str, value: &str, label_width: usize) {
        if self.width == 0 {
            self.lines.push(Line::from(Span::styled("", self.style)));
            return;
        }
        let value_start = label_width.saturating_add(2);
        if self.width >= SIDE_BY_SIDE_MIN_WIDTH && value_start < self.width {
            let mut values = wrap_line_at_whitespace(value, self.width - value_start).into_iter();
            let first = values.next().unwrap_or_default();
            self.lines.push(Line::from(vec![
                Span::styled(
                    // Pad by display width, as the column is measured.
                    format!(
                        "  {label}{}",
                        " ".repeat(label_width.saturating_sub(label.width()))
                    ),
                    self.style.add_modifier(Modifier::DIM),
                ),
                Span::styled(first.to_owned(), self.style),
            ]));
            self.lines.extend(values.map(|value| {
                Line::from(vec![
                    Span::styled(" ".repeat(value_start), self.style),
                    Span::styled(value.trim_start().to_string(), self.style),
                ])
            }));
        } else {
            self.lines.push(Line::from(Span::styled(
                truncate_one_line(&format!("  {label}"), self.width),
                self.style.add_modifier(Modifier::DIM),
            )));
            let indent_width = 4.min(self.width.saturating_sub(1));
            let value_width = self.width.saturating_sub(indent_width).max(1);
            let indent = " ".repeat(indent_width);
            self.lines
                .extend(
                    wrap_line_at_whitespace(value, value_width)
                        .into_iter()
                        .map(|value| {
                            Line::from(Span::styled(
                                format!("{indent}{}", value.trim_start()),
                                self.style,
                            ))
                        }),
                );
        }
    }

    pub(super) fn push_note(&mut self, note: &str) {
        if self.width == 0 {
            self.open
                .push(Row::Note(vec![Line::from(Span::styled("", self.style))]));
            return;
        }
        let indent_width = 2.min(self.width.saturating_sub(1));
        let note_width = self.width.saturating_sub(indent_width).max(1);
        let indent = " ".repeat(indent_width);
        let lines = wrap_line_at_whitespace(note, note_width)
            .into_iter()
            .map(|part| {
                Line::from(Span::styled(
                    format!("{indent}{}", part.trim_start()),
                    self.style.add_modifier(Modifier::DIM | Modifier::ITALIC),
                ))
            })
            .collect();
        self.open.push(Row::Note(lines));
    }

    pub(super) fn finish(mut self) -> Vec<Line<'static>> {
        self.close_section();
        debug_assert!(self.lines.iter().all(|line| line.width() <= self.width));
        fill_lines(self.lines, self.width, self.style)
    }
}

pub(super) fn fill_lines(
    lines: Vec<Line<'static>>,
    width: usize,
    style: Style,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|mut line| {
            let padding = width.saturating_sub(line.width());
            if padding > 0 {
                line.spans.push(Span::styled(" ".repeat(padding), style));
            }
            line
        })
        .collect()
}

#[cfg(test)]
#[path = "command_block_tests.rs"]
mod tests;
