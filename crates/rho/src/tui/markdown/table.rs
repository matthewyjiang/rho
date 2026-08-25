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

pub(super) fn markdown_table_line_count(lines: &[&str]) -> Option<usize> {
    parse_markdown_table(lines).map(|(_, consumed_lines)| consumed_lines)
}

fn parse_markdown_table(lines: &[&str]) -> Option<(MarkdownTable, usize)> {
    let [header, separator, ..] = lines else {
        return None;
    };
    if !has_markdown_table_delimiter(header) || !has_markdown_table_delimiter(separator) {
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
        if !has_markdown_table_delimiter(line) || line.trim().is_empty() {
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

fn has_markdown_table_delimiter(line: &str) -> bool {
    !markdown_table_delimiters(line).is_empty()
}

pub(super) fn markdown_table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let delimiters = markdown_table_delimiters(trimmed);
    let start = usize::from(delimiters.first().is_some_and(|delimiter| *delimiter == 0));
    let end = (trimmed.len()
        - usize::from(
            delimiters
                .last()
                .is_some_and(|delimiter| *delimiter + '|'.len_utf8() == trimmed.len()),
        ))
    .max(start);
    let delimiters = delimiters
        .into_iter()
        .filter(|delimiter| *delimiter >= start && *delimiter < end);

    let mut cells = Vec::new();
    let mut cell_start = start;
    for delimiter in delimiters {
        cells.push(markdown_table_cell(&trimmed[cell_start..delimiter]));
        cell_start = delimiter + '|'.len_utf8();
    }
    cells.push(markdown_table_cell(&trimmed[cell_start..end]));
    cells
}

fn markdown_table_delimiters(line: &str) -> Vec<usize> {
    let mut delimiters = Vec::new();
    let mut index = 0;
    let mut code_fence_len = None;

    while index < line.len() {
        let ch = line[index..]
            .chars()
            .next()
            .expect("index is within the line");
        if ch == '`' {
            let fence_len = line[index..]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count();
            match code_fence_len {
                Some(opening_fence_len) if opening_fence_len == fence_len => code_fence_len = None,
                None if has_matching_code_fence(line, index + fence_len, fence_len) => {
                    code_fence_len = Some(fence_len)
                }
                Some(_) | None => {}
            }
            index += fence_len;
        } else if ch == '\\' && code_fence_len.is_none() {
            index += ch.len_utf8();
            if let Some(next) = line[index..].chars().next() {
                index += next.len_utf8();
            }
        } else {
            if ch == '|' && code_fence_len.is_none() {
                delimiters.push(index);
            }
            index += ch.len_utf8();
        }
    }

    delimiters
}

fn has_matching_code_fence(line: &str, mut index: usize, fence_len: usize) -> bool {
    while index < line.len() {
        if line[index..].starts_with('`') {
            let candidate_len = line[index..]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count();
            if candidate_len == fence_len {
                return true;
            }
            index += candidate_len;
        } else {
            index += line[index..]
                .chars()
                .next()
                .expect("index is within the line")
                .len_utf8();
        }
    }
    false
}

fn markdown_table_cell(cell: &str) -> String {
    let mut result = String::new();
    let mut index = 0;
    let mut code_fence_len = None;

    while index < cell.len() {
        let ch = cell[index..]
            .chars()
            .next()
            .expect("index is within the cell");
        if ch == '`' {
            let fence_len = cell[index..]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count();
            match code_fence_len {
                Some(opening_fence_len) if opening_fence_len == fence_len => code_fence_len = None,
                None if has_matching_code_fence(cell, index + fence_len, fence_len) => {
                    code_fence_len = Some(fence_len)
                }
                Some(_) | None => {}
            }
            result.push_str(&cell[index..index + fence_len]);
            index += fence_len;
        } else if ch == '\\' && code_fence_len.is_none() {
            let next_index = index + ch.len_utf8();
            if cell[next_index..].starts_with('|') {
                result.push('|');
                index = next_index + '|'.len_utf8();
            } else {
                result.push(ch);
                index = next_index;
            }
        } else {
            result.push(ch);
            index += ch.len_utf8();
        }
    }

    result.trim().to_string()
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
    let column_widths = column_widths_for(table, width)?;
    Some(paint_table(table, &column_widths).lines)
}

fn column_widths_for(table: &MarkdownTable, width: usize) -> Option<Vec<usize>> {
    let column_count = table.alignments.len();
    let border_width = column_count + 1;
    let padding_width = column_count * 2;
    let available_content_width = width.checked_sub(border_width + padding_width)?;
    if available_content_width < column_count {
        return None;
    }

    let styled_rows = styled_table_rows(table);
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
    Some(column_widths)
}

fn styled_table_rows(table: &MarkdownTable) -> Vec<Vec<Vec<StyledSegment>>> {
    table
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| markdown_inline_segments(cell))
                .collect::<Vec<_>>()
        })
        .collect()
}

struct PaintedTable {
    lines: Vec<Line<'static>>,
    header_visual_len: usize,
}

fn paint_table(table: &MarkdownTable, column_widths: &[usize]) -> PaintedTable {
    let styled_rows = styled_table_rows(table);
    let mut lines = vec![table_border(column_widths, '┌', '┬', '┐')];
    let mut header_visual_len = lines.len();
    for (row_index, row) in styled_rows.iter().enumerate() {
        lines.extend(paint_table_row(
            row,
            column_widths,
            &table.alignments,
            row_index == 0,
        ));
        if row_index == 0 {
            lines.push(table_border(column_widths, '├', '┼', '┤'));
            header_visual_len = lines.len();
        }
    }
    lines.push(table_border(column_widths, '└', '┴', '┘'));
    PaintedTable {
        lines,
        header_visual_len,
    }
}

fn paint_table_row(
    row: &[Vec<StyledSegment>],
    column_widths: &[usize],
    alignments: &[TableAlignment],
    bold: bool,
) -> Vec<Line<'static>> {
    let wrapped_cells = row
        .iter()
        .zip(column_widths)
        .map(|(cell, column_width)| wrap_styled_segments(cell, *column_width))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut lines = Vec::with_capacity(row_height);
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
            let (left_padding, right_padding) = table_cell_padding(alignments[column], remaining);
            spans.push(Span::styled(" ".repeat(left_padding), Theme::text()));
            spans.extend(cell_spans.into_iter().map(|span| {
                if bold {
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
    lines
}

/// Snapshot of a trailing markdown table used to append rows without a reflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct StreamingTable {
    pub column_widths: Vec<usize>,
    pub consumed_source_len: usize,
    pub data_row_count: usize,
    pub header_visual_len: usize,
}

pub(in crate::tui) fn streaming_table(source: &str, width: usize) -> Option<StreamingTable> {
    let (table, consumed_source_len) = parse_table_from_source(source)?;
    let column_widths = column_widths_for(&table, width)?;
    let painted = paint_table(&table, &column_widths);
    Some(StreamingTable {
        column_widths,
        consumed_source_len,
        data_row_count: table.rows.len().saturating_sub(1),
        header_visual_len: painted.header_visual_len,
    })
}

pub(in crate::tui) fn render_streaming_table(
    source: &str,
    _width: usize,
    column_widths: &[usize],
) -> Option<Vec<Line<'static>>> {
    let (table, _) = parse_table_from_source(source)?;
    if table.alignments.len() != column_widths.len() {
        return None;
    }
    Some(paint_table(&table, column_widths).lines)
}

pub(in crate::tui) fn render_streaming_table_data_row(
    source: &str,
    _width: usize,
    column_widths: &[usize],
    data_row_index: usize,
) -> Option<Vec<Line<'static>>> {
    let (table, _) = parse_table_from_source(source)?;
    if table.alignments.len() != column_widths.len() {
        return None;
    }
    let row = table.rows.get(data_row_index.saturating_add(1))?;
    let styled = row
        .iter()
        .map(|cell| markdown_inline_segments(cell))
        .collect::<Vec<_>>();
    Some(paint_table_row(
        &styled,
        column_widths,
        &table.alignments,
        false,
    ))
}

pub(in crate::tui) fn streaming_table_bottom_border(column_widths: &[usize]) -> Line<'static> {
    table_border(column_widths, '└', '┴', '┘')
}

fn parse_table_from_source(source: &str) -> Option<(MarkdownTable, usize)> {
    let lines = source.lines().collect::<Vec<_>>();
    let (table, consumed_lines) = parse_markdown_table(&lines)?;
    let consumed_source_len = source
        .split_inclusive('\n')
        .take(consumed_lines)
        .map(str::len)
        .sum();
    Some((table, consumed_source_len))
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
