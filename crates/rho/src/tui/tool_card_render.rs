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
        slice_spans_by_bytes, soft_wrap_visible_ranges, spans_display_width, styled_blank_line,
        wrap_line_at_whitespace_ranges, wrap_line_hard, LineFill,
    },
    theme::Theme,
    tool_diff, ToolEntry,
};

/// First-line mid branch: `  ├ `.
const TREE_CHILD_MID: &str = "  ├ ";
/// First-line end branch: `  └ `.
const TREE_CHILD_END: &str = "  └ ";
/// Vertical stem on wrapped header/child rows; same box-drawing family as ├ / └.
/// Display width matches `  ├ ` / `  └ ` so wrapped content stays in one column.
const TREE_WRAP_STEM: &str = "  │ ";
/// Content column after `  ├ ` / `  └ `.
const CHILD_CONTENT_INDENT: &str = "    ";
/// Space hang under `└ ` when a last child wraps (trunk ends at └).
const TREE_CHILD_HANG: &str = CHILD_CONTENT_INDENT;

/// One fact or body/diff block rendered to terminal rows.
///
/// Tree facts always render mid trunk (`├` / `│`); last-child `└` / hang is
/// applied after budget clipping. Body/diff groups never take tree glyphs.
enum ChildGroup {
    /// Fact rows: `[0]` is the branch, `[1..]` are wrap stems.
    TreeFact(Vec<Line<'static>>),
    /// Body/diff rows with fixed content indent only.
    Plain(Vec<Line<'static>>),
}

impl ChildGroup {
    fn len(&self) -> usize {
        match self {
            Self::TreeFact(lines) | Self::Plain(lines) => lines.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn truncate(&mut self, len: usize) {
        match self {
            Self::TreeFact(lines) | Self::Plain(lines) => lines.truncate(len),
        }
    }

    fn into_lines(self) -> Vec<Line<'static>> {
        match self {
            Self::TreeFact(lines) | Self::Plain(lines) => lines,
        }
    }
}

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
    let total_rows: usize = children.iter().map(ChildGroup::len).sum();
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
    // For each group, whether a later TreeFact still needs the trunk. Mid
    // branches stay ├ and plain rows between them keep │ so multi-file File
    // headers connect through their body content.
    let later_has_tree = {
        let mut later = false;
        let mut flags = vec![false; emitted.len()];
        for (index, group) in emitted.iter().enumerate().rev() {
            flags[index] = later;
            if matches!(group, ChildGroup::TreeFact(_)) {
                later = true;
            }
        }
        flags
    };
    for (index, mut group) in emitted.into_iter().enumerate() {
        match &mut group {
            ChildGroup::TreeFact(fact_lines) => {
                // Last tree branch uses └ even when plain body hangs under it.
                // A following expand/collapse prompt keeps mid ├, matching facts.
                let is_last_tree = !later_has_tree[index] && !has_prompt;
                rewrite_tree_fact(fact_lines, is_last_tree);
            }
            ChildGroup::Plain(plain_lines) if later_has_tree[index] => {
                apply_trunk_stem(plain_lines);
            }
            ChildGroup::Plain(_) => {}
        }
        lines.extend(group.into_lines());
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
    let total_rows: usize = render_child_groups(card, width)
        .iter()
        .map(ChildGroup::len)
        .sum();
    total_rows > budget
}

/// Render each fact/body item into its full terminal-row group at `width`.
fn render_child_groups(card: &ToolCard, width: usize) -> Vec<ChildGroup> {
    let mut groups = Vec::new();
    for fact in &card.facts {
        // Always mid trunk here; last-child └ / hang is rewritten after clip.
        groups.push(ChildGroup::TreeFact(push_wrapped_tree_fact(
            fact_spans(fact),
            width,
        )));
    }
    match &card.body {
        ToolBody::None => {}
        ToolBody::Lines(body) => {
            for line in tool_diff::logical_lines(body) {
                let mut lines = Vec::new();
                push_body_line(&mut lines, &line, width, Theme::text());
                groups.push(ChildGroup::Plain(lines));
            }
        }
        ToolBody::Diff(rows) => {
            let gutter = tool_diff::gutter_width(rows);
            for row in rows {
                // Multi-file section headers keep the same ├ / └ trunk as
                // DiffStat facts so the card still reads as a connected tree
                // after path+stats moved onto the File row.
                if row.kind == DiffRowKind::File {
                    let spans = multi_file_header_spans(&row.text).unwrap_or_else(|| {
                        vec![Span::styled(row.text.clone(), Theme::tool_path())]
                    });
                    groups.push(ChildGroup::TreeFact(push_wrapped_tree_fact(spans, width)));
                    continue;
                }
                let mut lines = Vec::new();
                push_diff_row(&mut lines, row, gutter, width);
                groups.push(ChildGroup::Plain(lines));
            }
        }
    }
    groups
}

/// Final visible tree fact becomes └ with a space hang on wrap so the trunk
/// does not continue past the end. `[0]` is the branch; `[1..]` are wrap stems.
fn rewrite_tree_fact(lines: &mut [Line<'static>], is_last: bool) {
    let Some((first, rest)) = lines.split_first_mut() else {
        return;
    };
    set_tree_prefix(
        first,
        if is_last {
            TREE_CHILD_END
        } else {
            TREE_CHILD_MID
        },
    );
    let continuation = if is_last {
        TREE_CHILD_HANG
    } else {
        TREE_WRAP_STEM
    };
    for line in rest {
        set_tree_prefix(line, continuation);
    }
}

/// Fact rows keep the tree glyph in the first span; replace that span only.
fn set_tree_prefix(line: &mut Line<'static>, prefix: &str) {
    let Some(first) = line.spans.first_mut() else {
        return;
    };
    first.content = prefix.to_string().into();
}

/// Swap the plain content indent for a continuing trunk stem (`  │ `).
///
/// Body/diff rows start with [`CHILD_CONTENT_INDENT`] (or a hang of spaces that
/// begins with that indent width). When a later tree branch still follows,
/// replace that leading indent so the File headers read as one connected tree.
fn apply_trunk_stem(lines: &mut [Line<'static>]) {
    let indent = CHILD_CONTENT_INDENT;
    let indent_len = indent.len();
    for line in lines {
        let Some(first) = line.spans.first_mut() else {
            continue;
        };
        let text = first.content.as_ref();
        if text.starts_with(indent) {
            first.content = format!("{TREE_WRAP_STEM}{}", &text[indent_len..]).into();
            first.style = Theme::tool_tree();
            continue;
        }
        // Wrapped diff hangs use a pure-space first span wider than the indent.
        if text.len() >= indent_len && text.bytes().all(|b| b == b' ') {
            first.content = format!("{TREE_WRAP_STEM}{}", &text[indent_len..]).into();
            first.style = Theme::tool_tree();
        }
    }
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
                    push_wrapped_prefixed(
                        lines,
                        prefix,
                        wrappable,
                        width,
                        header_wrap_continuation_prefix,
                    );
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
                    push_wrapped_prefixed(
                        lines,
                        prefix,
                        wrappable,
                        width,
                        header_wrap_continuation_prefix,
                    );
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
                push_wrapped_prefixed(
                    lines,
                    prefix,
                    wrappable,
                    width,
                    header_wrap_continuation_prefix,
                );
            }
        }
    }
}

/// Wrap styled text under a fixed first-line prefix.
///
/// Continuations are supplied by `continuation_prefix(hang)`. Headers pad a
/// `│` stem out to the primary hang; tree facts use a plain `│` trunk
/// (last-child hang is applied later by [`rewrite_tree_fact`]).
///
/// Intentional newlines in the wrappable text (multi-line bash, heredocs) are
/// hard breaks. Soft width-wrap still applies within each logical line. Without
/// this, `\n` has zero display width and ratatui drops the control char, so
/// `check\ngit` renders as `checkgit`.
fn push_wrapped_prefixed(
    lines: &mut Vec<Line<'static>>,
    prefix: Vec<Span<'static>>,
    wrappable: Vec<Span<'static>>,
    width: usize,
    continuation_prefix: impl Fn(usize) -> Vec<Span<'static>>,
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
        for range in soft_wrap_visible_ranges(
            logical_line,
            wrap_line_at_whitespace_ranges(logical_line, content_width),
        ) {
            let chunk_spans =
                slice_spans_by_bytes(&wrappable, line_start + range.start, line_start + range.end);
            let mut row = if row_index == 0 {
                prefix.clone()
            } else {
                continuation_prefix(hang)
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

/// Wrap a fact under provisional `├` / `│`. Last-child rewrite owns `└` / hang.
fn push_wrapped_tree_fact(wrappable: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let prefix = vec![Span::styled(TREE_CHILD_MID.to_string(), Theme::tool_tree())];
    push_wrapped_prefixed(&mut lines, prefix, wrappable, width, |_| {
        vec![Span::styled(TREE_WRAP_STEM.to_string(), Theme::tool_tree())]
    });
    lines
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
    // File rows are promoted to TreeFact in render_child_groups so they keep
    // the connecting trunk. Plain body fallback still paints the path.
    if row.kind == DiffRowKind::File {
        push_body_line(lines, &row.text, width, Theme::tool_path());
        return;
    }
    // Op locators and other annotations are not content lines; drop the sign
    // column so they read as headers rather than gap markers.
    if row.kind == DiffRowKind::Meta {
        push_body_line(lines, &row.text, width, Theme::tool_diff_text(row.kind));
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

/// Parse `+N -M | path` multi-file headers into colored spans.
fn multi_file_header_spans(text: &str) -> Option<Vec<Span<'static>>> {
    let (stats, path) = text.split_once(" | ")?;
    let (added, removed) = stats.split_once(' ')?;
    if !added.starts_with('+') || !removed.starts_with('-') {
        return None;
    }
    if !added[1..].bytes().all(|b| b.is_ascii_digit())
        || !removed[1..].bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    if path.is_empty() {
        return None;
    }
    Some(vec![
        Span::styled(added.to_string(), Theme::tool_stat_add()),
        Span::raw(" "),
        Span::styled(removed.to_string(), Theme::tool_stat_del()),
        Span::styled(" | ", Theme::tool_meta()),
        Span::styled(path.to_string(), Theme::tool_path()),
    ])
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
