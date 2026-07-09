use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use super::{display_width, markdown_inline_segments, wrap_styled_segments, StyledSegment, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableAlignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, PartialEq, Eq)]
struct MarkdownTable {
    alignments: Vec<TableAlignment>,
    rows: Vec<Vec<String>>,
}

pub(super) fn markdown_table_lines(
    lines: &[&str],
    width: usize,
) -> Option<(Vec<Line<'static>>, usize)> {
    let (table, consumed_lines) = parse_markdown_table(lines)?;
    render_markdown_table(&table, width).map(|lines| (lines, consumed_lines))
}

fn parse_markdown_table(lines: &[&str]) -> Option<(MarkdownTable, usize)> {
    let [header, separator, ..] = lines else {
        return None;
    };
    if !header.contains('|') || !separator.contains('|') {
        return None;
    }

    let header = markdown_table_cells(header);
    let separator = markdown_table_cells(separator);
    if header.len() < 2 || header.len() != separator.len() {
        return None;
    }
    let alignments = separator
        .iter()
        .map(|cell| markdown_table_alignment(cell))
        .collect::<Option<Vec<_>>>()?;

    let column_count = header.len();
    let mut rows = vec![header];
    let mut consumed_lines = 2;
    for line in &lines[2..] {
        if !line.contains('|') || line.trim().is_empty() {
            break;
        }
        let mut cells = markdown_table_cells(line);
        cells.resize(column_count, String::new());
        cells.truncate(column_count);
        rows.push(cells);
        consumed_lines += 1;
    }

    Some((MarkdownTable { alignments, rows }, consumed_lines))
}

fn markdown_table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let mut cells = vec![String::new()];
    let mut escaped = false;
    let mut in_code = false;
    for ch in inner.chars() {
        if escaped {
            if ch != '|' {
                cells.last_mut().expect("table has a cell").push('\\');
            }
            cells.last_mut().expect("table has a cell").push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '`' => {
                in_code = !in_code;
                cells.last_mut().expect("table has a cell").push(ch);
            }
            '|' if !in_code => cells.push(String::new()),
            _ => cells.last_mut().expect("table has a cell").push(ch),
        }
    }
    if escaped {
        cells.last_mut().expect("table has a cell").push('\\');
    }
    cells
        .into_iter()
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn markdown_table_alignment(cell: &str) -> Option<TableAlignment> {
    let cell = cell.trim();
    let left = cell.starts_with(':');
    let right = cell.ends_with(':');
    let marker = cell.trim_matches(':');
    (marker.len() >= 3 && marker.chars().all(|ch| ch == '-')).then_some(match (left, right) {
        (true, true) => TableAlignment::Center,
        (false, true) => TableAlignment::Right,
        (true, false) | (false, false) => TableAlignment::Left,
    })
}

fn render_markdown_table(table: &MarkdownTable, width: usize) -> Option<Vec<Line<'static>>> {
    let column_count = table.alignments.len();
    let border_width = column_count + 1;
    let padding_width = column_count * 2;
    let available_content_width = width.checked_sub(border_width + padding_width)?;
    if available_content_width < column_count {
        return None;
    }

    let styled_rows = table
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| markdown_inline_segments(cell))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut column_widths = (0..column_count)
        .map(|column| {
            styled_rows
                .iter()
                .map(|row| styled_segments_width(&row[column]))
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect::<Vec<_>>();
    while column_widths.iter().sum::<usize>() > available_content_width {
        let (column, _) = column_widths
            .iter()
            .enumerate()
            .filter(|(_, column_width)| **column_width > 1)
            .max_by_key(|(_, column_width)| **column_width)?;
        column_widths[column] -= 1;
    }

    let mut lines = vec![table_border(&column_widths, '┌', '┬', '┐')];
    for (row_index, row) in styled_rows.iter().enumerate() {
        let wrapped_cells = row
            .iter()
            .zip(&column_widths)
            .map(|(cell, column_width)| wrap_styled_segments(cell, *column_width))
            .collect::<Vec<_>>();
        let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
        for visual_line in 0..row_height {
            let mut spans = vec![Span::styled("│", Theme::dim())];
            for (column, cell_lines) in wrapped_cells.iter().enumerate() {
                spans.push(Span::styled(" ", Theme::text()));
                let cell_spans = cell_lines
                    .get(visual_line)
                    .map(|line| line.spans.clone())
                    .unwrap_or_else(|| vec![Span::raw(String::new())]);
                let cell_width = spans_display_width(&cell_spans);
                let remaining = column_widths[column].saturating_sub(cell_width);
                let (left_padding, right_padding) =
                    table_cell_padding(table.alignments[column], remaining);
                spans.push(Span::styled(" ".repeat(left_padding), Theme::text()));
                spans.extend(cell_spans.into_iter().map(|span| {
                    if row_index == 0 {
                        Span::styled(
                            span.content.into_owned(),
                            span.style.add_modifier(Modifier::BOLD),
                        )
                    } else {
                        span
                    }
                }));
                spans.push(Span::styled(" ".repeat(right_padding + 1), Theme::text()));
                spans.push(Span::styled("│", Theme::dim()));
            }
            lines.push(Line::from(spans));
        }
        if row_index == 0 {
            lines.push(table_border(&column_widths, '├', '┼', '┤'));
        }
    }
    lines.push(table_border(&column_widths, '└', '┴', '┘'));
    Some(lines)
}

fn styled_segments_width(segments: &[StyledSegment]) -> usize {
    segments
        .iter()
        .map(|segment| display_width(&segment.text))
        .sum()
}

fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

fn table_cell_padding(alignment: TableAlignment, remaining: usize) -> (usize, usize) {
    match alignment {
        TableAlignment::Left => (0, remaining),
        TableAlignment::Center => (remaining / 2, remaining - remaining / 2),
        TableAlignment::Right => (remaining, 0),
    }
}

fn table_border(column_widths: &[usize], left: char, junction: char, right: char) -> Line<'static> {
    let mut border = left.to_string();
    for (index, column_width) in column_widths.iter().enumerate() {
        border.push_str(&"─".repeat(column_width + 2));
        border.push(if index + 1 == column_widths.len() {
            right
        } else {
            junction
        });
    }
    Line::from(Span::styled(border, Theme::dim()))
}
