//! Multi-span Call + Children rendering for structured tool cards.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolHeader, ToolStatus,
};
use unicode_width::UnicodeWidthStr;

use super::{
    feed_image::reserve_optional_image_rows,
    render::{
        display_width, pad_display_line, padded_content_width, push_wrapped_text,
        slice_spans_by_bytes, spans_display_width, styled_blank_line,
        wrap_line_at_whitespace_ranges, wrap_line_hard, LineFill,
    },
    theme::Theme,
    tool_diff, ToolEntry,
};

const TREE_INDENT: &str = "  ";
const TREE_BRANCH_MID: &str = "├ ";
const TREE_BRANCH_END: &str = "└ ";
/// Space hang under `└ ` when a last child wraps (trunk ends at └).
const TREE_CONTINUE: &str = "  ";
/// Vertical stem on wrapped header/child rows; same box-drawing family as ├ / └.
/// Display width matches `  ├ ` / `  └ ` so wrapped content stays in one column.
const TREE_WRAP_STEM: &str = "  │ ";
/// Content column after `  ├ ` / `  └ `.
const CHILD_CONTENT_INDENT: &str = "    ";

pub(super) fn tool_entry_lines(
    tool: &ToolEntry,
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    let inner_width = padded_content_width(width);
    let mut lines = Vec::new();
    push_tool_card(
        &mut lines,
        &tool.card,
        inner_width,
        max_tool_output_lines,
        tool.expanded,
    );
    reserve_optional_image_rows(&mut lines, tool.image.as_ref(), width);
    // One trailing spacer only. Prior entries own the blank above this card.
    let padding_style = Theme::tool_card_padding();
    let mut padded = Vec::with_capacity(lines.len() + 1);
    padded.extend(lines.into_iter().map(pad_display_line));
    padded.push(styled_blank_line(width, padding_style));
    padded
}

pub(super) fn push_tool_card(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCard,
    width: usize,
    max_tool_output_lines: usize,
    expanded: bool,
) {
    push_header_line(lines, card, card.status, width);

    let budget = max_tool_output_lines.max(1);
    let children = render_child_groups(card, width);
    let total_rows: usize = children.iter().map(Vec::len).sum();
    let show_collapse_prompt = expanded && total_rows > budget;
    let mut remaining = if expanded { usize::MAX } else { budget };
    let mut hidden_rows = 0usize;
    let mut emitted = Vec::new();

    for group in children {
        if remaining == 0 {
            hidden_rows = hidden_rows.saturating_add(group.len());
            continue;
        }
        if group.len() <= remaining {
            remaining = remaining.saturating_sub(group.len());
            emitted.push(group);
            continue;
        }
        // Clip a wrapping child to the remaining terminal-row budget.
        hidden_rows = hidden_rows.saturating_add(group.len() - remaining);
        let mut clipped = group;
        clipped.truncate(remaining);
        remaining = 0;
        if !clipped.is_empty() {
            emitted.push(clipped);
        }
    }

    let show_expand_prompt = !expanded && hidden_rows > 0;
    let has_prompt = show_expand_prompt || show_collapse_prompt;
    let last_child = emitted.len().saturating_sub(1);
    for (index, mut group) in emitted.into_iter().enumerate() {
        let is_last_child = index == last_child && !has_prompt;
        rewrite_child_group_branches(&mut group, is_last_child);
        lines.extend(group);
    }

    if show_expand_prompt {
        let prompt = format!("... {hidden_rows} more lines, ctrl+o to expand");
        push_wrapped_text(lines, &prompt, width, Theme::dim(), LineFill::PadToWidth);
    } else if show_collapse_prompt {
        push_wrapped_text(
            lines,
            "ctrl+o to collapse",
            width,
            Theme::dim(),
            LineFill::PadToWidth,
        );
    }
}

/// Whether ctrl+o / click should toggle this tool at the given terminal width.
pub(super) fn card_is_toggleable(
    card: &ToolCard,
    width: usize,
    max_tool_output_lines: usize,
    _expanded: bool,
) -> bool {
    let budget = max_tool_output_lines.max(1);
    let total_rows: usize = render_child_groups(card, width).iter().map(Vec::len).sum();
    total_rows > budget
}

/// Render each fact/body item into its full terminal-row group at `width`.
fn render_child_groups(card: &ToolCard, width: usize) -> Vec<Vec<Line<'static>>> {
    let mut groups = Vec::new();
    for fact in &card.facts {
        // Branch glyph is rewritten after budget clipping once last-child is known.
        let mut lines = Vec::new();
        push_fact_line(&mut lines, fact, /*is_last*/ false, width);
        groups.push(lines);
    }
    match &card.body {
        ToolBody::None => {}
        ToolBody::Lines(body) => {
            for line in tool_diff::logical_lines(body) {
                let mut lines = Vec::new();
                push_body_line(&mut lines, &line, width, Theme::text());
                groups.push(lines);
            }
        }
        ToolBody::Diff(rows) => {
            let gutter = tool_diff::gutter_width(rows);
            for row in rows {
                let mut lines = Vec::new();
                push_diff_row(&mut lines, row, gutter, width);
                groups.push(lines);
            }
        }
    }
    groups
}

/// Tree-fact groups draw ├ / │ by default; the final visible fact becomes └
/// with a space hang on wrap so the trunk does not continue past the end.
/// Body/diff groups have no branch glyphs and are left alone.
fn rewrite_child_group_branches(group: &mut [Line<'static>], is_last: bool) {
    let Some(first_line) = group.first_mut() else {
        return;
    };
    if !line_starts_with_tree_branch(first_line) {
        return;
    }
    rewrite_fact_branch(first_line, is_last);
    for line in group.iter_mut().skip(1) {
        rewrite_wrap_stem(line, is_last);
    }
}

fn line_starts_with_tree_branch(line: &Line<'static>) -> bool {
    let Some(first) = line.spans.first() else {
        return false;
    };
    let content = first.content.as_ref();
    let mid = format!("{TREE_INDENT}{TREE_BRANCH_MID}");
    let end = format!("{TREE_INDENT}{TREE_BRANCH_END}");
    content.starts_with(&mid) || content.starts_with(&end)
}

fn rewrite_fact_branch(line: &mut Line<'static>, is_last: bool) {
    let Some(first) = line.spans.first_mut() else {
        return;
    };
    let content = first.content.as_ref();
    let mid = format!("{TREE_INDENT}{TREE_BRANCH_MID}");
    let end = format!("{TREE_INDENT}{TREE_BRANCH_END}");
    if content.starts_with(&mid) || content.starts_with(&end) {
        let suffix = &content[mid.len().min(content.len())..];
        first.content = format!(
            "{}{suffix}",
            if is_last {
                format!("{TREE_INDENT}{TREE_BRANCH_END}")
            } else {
                format!("{TREE_INDENT}{TREE_BRANCH_MID}")
            }
        )
        .into();
    }
}

fn rewrite_wrap_stem(line: &mut Line<'static>, is_last: bool) {
    let Some(first) = line.spans.first_mut() else {
        return;
    };
    let content = first.content.as_ref();
    let mid = TREE_WRAP_STEM;
    let end = format!("{TREE_INDENT}{TREE_CONTINUE}");
    if content != mid && content != end.as_str() {
        return;
    }
    first.content = if is_last { end.into() } else { mid.into() };
}

fn push_header_line(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCard,
    status: ToolStatus,
    width: usize,
) {
    // Marker stays on the first row only. Primary/command/detail may wrap with a
    // hang under the fixed prefix so long streamed args stay visible (main used
    // to hard-wrap whole tool lines; a single clipped header hides the tail).
    let marker = Span::styled(format!("{} ", status.marker()), Theme::tool_marker(status));
    match &card.header {
        ToolHeader::Call { verb, primary } => {
            let mut prefix = vec![
                marker,
                Span::styled(verb.clone(), Theme::tool_verb(card.family)),
            ];
            match primary.as_ref().filter(|primary| !primary.is_empty()) {
                Some(primary) => {
                    prefix.push(Span::styled("(", Theme::tool_primary()));
                    let wrappable = vec![
                        Span::styled(primary.clone(), Theme::tool_primary()),
                        Span::styled(")", Theme::tool_primary()),
                    ];
                    push_wrapped_header(lines, prefix, wrappable, width);
                }
                None => lines.push(pad_spans_line(prefix, width)),
            }
        }
        ToolHeader::Shell { prompt, command } => {
            let mut prefix = vec![
                marker,
                Span::styled(prompt.clone(), Theme::tool_verb(card.family)),
            ];
            match command.as_ref().filter(|command| !command.is_empty()) {
                Some(command) => {
                    prefix.push(Span::raw(" "));
                    let wrappable = vec![Span::styled(command.clone(), Theme::tool_primary())];
                    push_wrapped_header(lines, prefix, wrappable, width);
                }
                None => lines.push(pad_spans_line(prefix, width)),
            }
        }
        ToolHeader::StatusFirst { identity, detail } => {
            let mut prefix = vec![
                marker,
                Span::styled(identity.clone(), Theme::tool_verb(card.family)),
            ];
            if detail.is_empty() {
                lines.push(pad_spans_line(prefix, width));
            } else {
                prefix.push(Span::raw("  "));
                let wrappable = vec![Span::styled(detail.clone(), Theme::text())];
                push_wrapped_header(lines, prefix, wrappable, width);
            }
        }
    }
}

/// Wrap header primary/command under a fixed first-line prefix.
///
/// Continuations draw a tree-column `│` stem, then pad to the primary hang so
/// children (`├` / `└`) still read as a connected trunk under the call.
///
/// Intentional newlines in the wrappable text (multi-line bash, heredocs) are
/// hard breaks. Soft width-wrap still applies within each logical line. Without
/// this, `\n` has zero display width and ratatui drops the control char, so
/// `check\ngit` renders as `checkgit`.
fn push_wrapped_header(
    lines: &mut Vec<Line<'static>>,
    prefix: Vec<Span<'static>>,
    wrappable: Vec<Span<'static>>,
    width: usize,
) {
    let hang = spans_display_width(&prefix);
    if hang >= width {
        // Pathological narrow width: fall back to a single padded row.
        let mut spans = prefix;
        spans.extend(wrappable);
        lines.push(pad_spans_line(spans, width));
        return;
    }
    let content_width = (width - hang).max(1);
    let text: String = wrappable.iter().map(|span| span.content.as_ref()).collect();
    if text.is_empty() {
        lines.push(pad_spans_line(prefix, width));
        return;
    }

    // Same hard-then-soft pattern as push_wrapped_text_with: str::lines() first.
    let mut row_index = 0usize;
    for logical_line in text.lines() {
        let line_start = subslice_start(&text, logical_line);
        for (wrap_index, range) in wrap_line_at_whitespace_ranges(logical_line, content_width)
            .into_iter()
            .enumerate()
        {
            let mut start = range.start;
            let end = range.end;
            if wrap_index > 0 {
                // Soft-wrap only: keep hang indent when a break leaves spaces.
                // Hard newline rows keep their own leading indentation.
                while start < end {
                    let ch = logical_line[start..].chars().next().expect("start < end");
                    if !ch.is_whitespace() {
                        break;
                    }
                    start += ch.len_utf8();
                }
                if start >= end {
                    continue;
                }
            }
            let chunk_spans =
                slice_spans_by_bytes(&wrappable, line_start + start, line_start + end);
            let mut row = if row_index == 0 {
                prefix.clone()
            } else {
                header_wrap_continuation_prefix(hang)
            };
            row.extend(chunk_spans);
            lines.push(pad_spans_line(row, width));
            row_index += 1;
        }
    }
}

/// Byte offset of `child` inside `parent`. `child` must be a subslice of `parent`
/// (as yielded by `str::lines()`).
fn subslice_start(parent: &str, child: &str) -> usize {
    let start = child.as_ptr() as usize - parent.as_ptr() as usize;
    debug_assert!(parent.get(start..start + child.len()) == Some(child));
    start
}

/// `  │ ` in the tree column, then spaces out to the primary hang.
fn header_wrap_continuation_prefix(hang: usize) -> Vec<Span<'static>> {
    let stem_width = display_width(TREE_WRAP_STEM);
    let mut spans = vec![Span::styled(TREE_WRAP_STEM.to_string(), Theme::tool_tree())];
    if hang > stem_width {
        spans.push(Span::styled(" ".repeat(hang - stem_width), Theme::text()));
    }
    spans
}

fn push_fact_line(lines: &mut Vec<Line<'static>>, fact: &ToolFact, is_last: bool, width: usize) {
    let branch = if is_last {
        TREE_BRANCH_END
    } else {
        TREE_BRANCH_MID
    };
    let prefix = vec![Span::styled(
        format!("{TREE_INDENT}{branch}"),
        Theme::tool_tree(),
    )];
    // Soft-wrap like headers and keep a `│` trunk on continuations so long
    // search queries and other fact text stay readable on every tool card.
    push_wrapped_child(
        lines,
        prefix,
        fact_spans(fact),
        /*is_last*/ is_last,
        width,
    );
}

/// Wrap child content under a fixed first-line tree prefix.
///
/// Continuations use `│` while more siblings follow, or a space hang after `└`
/// so the trunk ends with the last child. Matches header wrap grammar.
fn push_wrapped_child(
    lines: &mut Vec<Line<'static>>,
    prefix: Vec<Span<'static>>,
    wrappable: Vec<Span<'static>>,
    is_last: bool,
    width: usize,
) {
    let hang = spans_display_width(&prefix);
    if hang >= width {
        let mut spans = prefix;
        spans.extend(wrappable);
        lines.push(pad_spans_line(spans, width));
        return;
    }
    let content_width = (width - hang).max(1);
    let text: String = wrappable.iter().map(|span| span.content.as_ref()).collect();
    if text.is_empty() {
        lines.push(pad_spans_line(prefix, width));
        return;
    }

    let mut row_index = 0usize;
    for logical_line in text.lines() {
        let line_start = subslice_start(&text, logical_line);
        for (wrap_index, range) in wrap_line_at_whitespace_ranges(logical_line, content_width)
            .into_iter()
            .enumerate()
        {
            let mut start = range.start;
            let end = range.end;
            if wrap_index > 0 {
                while start < end {
                    let ch = logical_line[start..].chars().next().expect("start < end");
                    if !ch.is_whitespace() {
                        break;
                    }
                    start += ch.len_utf8();
                }
                if start >= end {
                    continue;
                }
            }
            let chunk_spans =
                slice_spans_by_bytes(&wrappable, line_start + start, line_start + end);
            let mut row = if row_index == 0 {
                prefix.clone()
            } else {
                child_wrap_continuation_prefix(is_last)
            };
            row.extend(chunk_spans);
            lines.push(pad_spans_line(row, width));
            row_index += 1;
        }
    }
}

fn child_wrap_continuation_prefix(is_last: bool) -> Vec<Span<'static>> {
    let stem = if is_last {
        format!("{TREE_INDENT}{TREE_CONTINUE}")
    } else {
        TREE_WRAP_STEM.to_string()
    };
    vec![Span::styled(stem, Theme::tool_tree())]
}

fn fact_spans(fact: &ToolFact) -> Vec<Span<'static>> {
    match fact {
        ToolFact::DiffStat {
            added,
            removed,
            path,
        } => {
            let mut spans = vec![
                Span::styled(format!("+{added}"), Theme::tool_stat_add()),
                Span::raw(" "),
                Span::styled(format!("-{removed}"), Theme::tool_stat_del()),
                Span::styled(" lines", Theme::tool_meta()),
            ];
            if let Some(path) = path.as_ref().filter(|path| !path.is_empty()) {
                spans.push(Span::styled(" | ", Theme::tool_meta()));
                spans.push(Span::styled(path.clone(), Theme::tool_path()));
            }
            spans
        }
        ToolFact::Exit { code, duration_ms } => {
            let status = if *code == 0 {
                ToolStatus::Ok
            } else {
                ToolStatus::Error
            };
            let mut spans = vec![Span::styled(
                format!("exit {code}"),
                Theme::tool_exit(status),
            )];
            if let Some(ms) = duration_ms {
                let secs = *ms as f64 / 1000.0;
                spans.push(Span::styled(format!(" · {secs:.1}s"), Theme::tool_meta()));
            }
            spans
        }
        ToolFact::Count {
            label,
            value,
            detail,
        } => {
            let mut text = format!("{value} {label}");
            if let Some(detail) = detail.as_ref().filter(|detail| !detail.is_empty()) {
                text.push(' ');
                text.push_str(detail);
            }
            vec![Span::styled(text, Theme::text())]
        }
        ToolFact::Meta { text } => vec![Span::styled(text.clone(), Theme::tool_meta())],
        ToolFact::Error { text } => vec![Span::styled(text.clone(), Theme::tool_error_text())],
        ToolFact::Progress { completed, total } => {
            let text = match total {
                Some(total) => format!("{completed}/{total}"),
                None => format!("{completed}"),
            };
            vec![Span::styled(text, Theme::tool_meta())]
        }
        ToolFact::Text { text } => vec![Span::styled(text.clone(), Theme::text())],
    }
}

/// Draw one diff row as `<indent><line no> <sign> <text>`.
///
/// The number gutter and sign column are fixed, so wrapped text hangs under the
/// text column and added/removed rows stay distinguishable without color.
fn push_diff_row(lines: &mut Vec<Line<'static>>, row: &DiffRow, gutter: usize, width: usize) {
    if row.kind == DiffRowKind::File {
        push_body_line(lines, &row.text, width, Theme::tool_path());
        return;
    }

    // Unnumbered bodies (patch text without hunk headers) drop the gutter and
    // its separator so the sign column sits right under the tree indent.
    let number = match (gutter, row.line) {
        (0, _) => String::new(),
        (_, Some(line)) => format!("{line:>gutter$} "),
        (_, None) => " ".repeat(gutter + 1),
    };
    let sign = format!("{} ", row.kind.sign());
    let prefix_width = display_width(CHILD_CONTENT_INDENT) + display_width(&number) + sign.len();
    let content_width = width.saturating_sub(prefix_width).max(1);
    let text_style = Theme::tool_diff_text(row.kind);

    let mut chunks = wrap_line_hard(&row.text, content_width);
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    for (index, chunk) in chunks.into_iter().enumerate() {
        let mut spans = if index == 0 {
            vec![
                Span::styled(
                    format!("{CHILD_CONTENT_INDENT}{number}"),
                    Theme::tool_diff_gutter(),
                ),
                Span::styled(sign.clone(), text_style),
            ]
        } else {
            vec![Span::styled(" ".repeat(prefix_width), Theme::tool_tree())]
        };
        spans.push(Span::styled(chunk, text_style));
        lines.push(pad_spans_line(spans, width));
    }
}

fn push_body_line(lines: &mut Vec<Line<'static>>, line: &str, width: usize, style: Style) {
    // Indent body under the tree content column.
    let prefix = CHILD_CONTENT_INDENT;
    let prefix_width = display_width(prefix);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let chunks = wrap_line_hard(line, content_width);
    if chunks.is_empty() {
        lines.push(pad_spans_line(
            vec![
                Span::styled(prefix.to_string(), Theme::tool_tree()),
                Span::styled(String::new(), style),
            ],
            width,
        ));
        return;
    }
    for chunk in chunks {
        lines.push(pad_spans_line(
            vec![
                Span::styled(prefix.to_string(), Theme::tool_tree()),
                Span::styled(chunk, style),
            ],
            width,
        ));
    }
}

fn pad_spans_line(mut spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), Theme::text()));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "tool_card_render_tests.rs"]
mod tests;
