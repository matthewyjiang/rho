use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{
    render::{truncate_one_line, wrap_line_at_whitespace},
    theme::Theme,
};

const FIELD_LABEL_WIDTH: usize = 14;
const SIDE_BY_SIDE_MIN_WIDTH: usize = 32;

pub(super) struct CommandBlock {
    lines: Vec<Line<'static>>,
    style: Style,
    width: usize,
}

impl CommandBlock {
    pub(super) fn new(width: usize) -> Self {
        Self {
            lines: Vec::new(),
            style: Theme::command_block(),
            width,
        }
    }

    pub(super) fn push_header(&mut self, title: &str, detail: &str) {
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
        self.lines.push(Line::from(Span::styled("", self.style)));
        self.lines.push(Line::from(Span::styled(
            truncate_one_line(title, self.width),
            self.style.add_modifier(Modifier::BOLD),
        )));
    }

    pub(super) fn push_field(&mut self, label: &str, value: &str) {
        if self.width == 0 {
            self.lines.push(Line::from(Span::styled("", self.style)));
            return;
        }
        let label_width = FIELD_LABEL_WIDTH.min(self.width.saturating_sub(1));
        let value_start = label_width.saturating_add(2);
        if self.width >= SIDE_BY_SIDE_MIN_WIDTH && value_start < self.width {
            let mut values = wrap_line_at_whitespace(value, self.width - value_start).into_iter();
            let first = values.next().unwrap_or_default();
            self.lines.push(Line::from(vec![
                Span::styled(
                    format!("  {label:label_width$}"),
                    self.style.add_modifier(Modifier::DIM),
                ),
                Span::styled(first, self.style),
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
            self.lines.push(Line::from(Span::styled("", self.style)));
            return;
        }
        let indent_width = 2.min(self.width.saturating_sub(1));
        let note_width = self.width.saturating_sub(indent_width).max(1);
        let indent = " ".repeat(indent_width);
        self.lines.extend(
            wrap_line_at_whitespace(note, note_width)
                .into_iter()
                .map(|part| {
                    Line::from(Span::styled(
                        format!("{indent}{}", part.trim_start()),
                        self.style.add_modifier(Modifier::DIM | Modifier::ITALIC),
                    ))
                }),
        );
    }

    pub(super) fn finish(self) -> Vec<Line<'static>> {
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
