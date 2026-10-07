//! Render JSON node outputs as a readable outline for the details pane.
//!
//! The outline is for reading, not round-tripping: objects become aligned
//! `key  value` rows, long strings become wrapped paragraphs under their key,
//! and arrays become bulleted (scalars) or numbered (objects) items. Within an
//! object, short facts come first, then paragraphs, then nested collections,
//! so a result's status and summary lead instead of following its lists.

use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

use super::super::{
    render::{display_width, wrap_line_at_whitespace},
    theme::Theme,
};

/// Indent added per nesting level.
const NEST_INDENT: usize = 2;
/// Receipt: keeps the value column usable in a ~45-column details pane while
/// fitting typical snake_case keys such as `changed_file_count` (18).
const MAX_KEY_COLUMN: usize = 20;
/// Marker for scalar list items.
const BULLET: &str = "• ";

/// Outline lines for `value` wrapped to `width` columns.
///
/// Every line starts with an indent span (possibly empty) so list markers can
/// be patched into the first line of an item without re-rendering it.
pub(super) fn outline_lines(value: &Value, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    push_value(&mut out, value, 0, width.max(1));
    out
}

fn push_value(out: &mut Vec<Line<'static>>, value: &Value, indent: usize, width: usize) {
    match value {
        Value::Object(map) if !map.is_empty() => push_object(out, map, indent, width),
        Value::Array(items) if !items.is_empty() => push_items(out, items, indent, width),
        scalar => push_paragraph(
            out,
            &scalar_text(scalar),
            scalar_style(scalar),
            indent,
            width,
        ),
    }
}

/// Display group for one object entry, in render order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EntryGroup {
    /// Fits on the key row.
    Inline,
    /// A string too long for the key row; wrapped below the key.
    Paragraph,
    /// A non-empty array or object rendered below the key.
    Nested,
}

fn push_object(
    out: &mut Vec<Line<'static>>,
    map: &Map<String, Value>,
    indent: usize,
    width: usize,
) {
    let key_column = map
        .iter()
        .filter(|(_, value)| is_scalar_like(value))
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or(0)
        .min(MAX_KEY_COLUMN);
    let mut entries = map
        .iter()
        .map(|(key, value)| {
            let group = entry_group(key, value, key_column, indent, width);
            (group, key, value)
        })
        .collect::<Vec<_>>();
    // Stable sort keeps the source key order within each group.
    entries.sort_by_key(|(group, _, _)| *group);

    for (group, key, value) in entries {
        match group {
            EntryGroup::Inline => {
                let pad = key_column.saturating_sub(display_width(key));
                out.push(Line::from(vec![
                    indent_span(indent),
                    Span::styled(key.clone(), Theme::accent()),
                    Span::raw(" ".repeat(pad + 2)),
                    Span::styled(scalar_text(value), scalar_style(value)),
                ]));
            }
            EntryGroup::Paragraph => {
                out.push(key_line(key, None, indent));
                let text = value.as_str().unwrap_or_default();
                push_paragraph(out, text, Theme::text(), indent + NEST_INDENT, width);
            }
            EntryGroup::Nested => {
                let count = value.as_array().map(Vec::len);
                out.push(key_line(key, count, indent));
                push_value(out, value, indent + NEST_INDENT, width);
            }
        }
    }
}

fn entry_group(
    key: &str,
    value: &Value,
    key_column: usize,
    indent: usize,
    width: usize,
) -> EntryGroup {
    if !is_scalar_like(value) {
        return EntryGroup::Nested;
    }
    let text = scalar_text(value);
    let row = indent + key_column.max(display_width(key)) + 2 + display_width(&text);
    if value.is_string() && (row > width || text.contains('\n')) {
        EntryGroup::Paragraph
    } else {
        EntryGroup::Inline
    }
}

fn push_items(out: &mut Vec<Line<'static>>, items: &[Value], indent: usize, width: usize) {
    let numbered = items.iter().any(|item| !is_scalar_like(item));
    let marker_width = if numbered {
        format!("{}. ", items.len()).len()
    } else {
        display_width(BULLET)
    };
    for (index, item) in items.iter().enumerate() {
        // Blank rows separate multi-line object items.
        if numbered && index > 0 {
            out.push(Line::from(indent_span(indent)));
        }
        let first = out.len();
        push_value(out, item, indent + marker_width, width);
        let marker = if numbered {
            format!("{:<marker_width$}", format!("{}.", index + 1))
        } else {
            BULLET.to_owned()
        };
        if let Some(line) = out.get_mut(first) {
            // Swap the item's leading blank columns for this marker. A nested
            // list already put its own marker after those columns; keep it.
            let inner = line.spans[0]
                .content
                .get(indent + marker_width..)
                .unwrap_or_default()
                .to_owned();
            line.spans[0] = Span::styled(
                format!("{}{marker}{inner}", " ".repeat(indent)),
                Theme::dim(),
            );
        }
    }
}

fn push_paragraph(
    out: &mut Vec<Line<'static>>,
    text: &str,
    style: ratatui::style::Style,
    indent: usize,
    width: usize,
) {
    let available = width.saturating_sub(indent).max(1);
    let mut emitted = false;
    for source_line in text.lines() {
        for chunk in wrap_line_at_whitespace(source_line, available) {
            out.push(Line::from(vec![
                indent_span(indent),
                Span::styled(chunk.to_owned(), style),
            ]));
            emitted = true;
        }
    }
    if !emitted {
        out.push(Line::from(vec![indent_span(indent), Span::raw("")]));
    }
}

fn key_line(key: &str, count: Option<usize>, indent: usize) -> Line<'static> {
    let mut spans = vec![
        indent_span(indent),
        Span::styled(key.to_owned(), Theme::accent()),
    ];
    if let Some(count) = count {
        spans.push(Span::styled(format!(" · {count}"), Theme::dim()));
    }
    Line::from(spans)
}

fn indent_span(indent: usize) -> Span<'static> {
    Span::raw(" ".repeat(indent))
}

/// Scalars and empty collections render as a single short value.
fn is_scalar_like(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => true,
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) if text.is_empty() => "(empty)".to_owned(),
        Value::String(text) => text.clone(),
        Value::Array(_) | Value::Object(_) => "none".to_owned(),
        Value::Null => "null".to_owned(),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
    }
}

fn scalar_style(value: &Value) -> ratatui::style::Style {
    match value {
        Value::String(text) if !text.is_empty() => Theme::text(),
        Value::Bool(_) | Value::Number(_) => Theme::text_strong(),
        Value::String(_) | Value::Array(_) | Value::Object(_) | Value::Null => Theme::dim(),
    }
}

#[cfg(test)]
#[path = "json_outline_tests.rs"]
mod tests;
