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
        display_width, hard_wrap_styled_spans, pad_display_line, padded_content_width,
        push_wrapped_text, slice_spans_by_bytes, soft_wrap_visible_ranges, spans_display_width,
        styled_blank_line, wrap_line_at_whitespace_ranges, wrap_line_hard, LineFill,
    },
    syntax::spans_from_segments_with_matches,
    theme::Theme,
    tool_diff::{self, DiffSyntax},
    tool_search::SearchSyntax,
    ToolEntry,
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
/// applied after budget clipping. Plain body rows start with a content indent;
/// when a later tree branch remains after clipping, that indent is rewritten
/// to a continuing trunk stem (`│`) so multi-file File headers stay connected.
enum ChildGroup {
    /// Fact / File-section rows: `[0]` is the branch, `[1..]` are wrap stems.
    TreeFact(Vec<Line<'static>>),
    /// Body/diff rows with fixed content indent (stem applied after clip when
    /// a later tree branch still follows).
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
    max_image_height: u16,
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
    reserve_optional_image_rows(&mut lines, tool.image.as_ref(), width, max_image_height);
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
    // Collapsed: paint only the visible budget (syntax is the costly part).
    // Expanded: paint the full body. Toggle checks never full-paint.
    let paint_budget = if expanded { None } else { Some(budget) };
    let rendered = render_child_groups(card, width, paint_budget);
    let total_rows = rendered.total_terminal_rows;
    let show_collapse_prompt = expanded && total_rows > budget;
    let mut remaining = if expanded { usize::MAX } else { budget };
    let mut hidden_rows = 0usize;
    let mut emitted = Vec::new();

    for group in rendered.groups {
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
    // When paint stopped early, estimated tail is already in total_rows.
    if !expanded {
        let shown: usize = emitted.iter().map(ChildGroup::len).sum();
        hidden_rows = total_rows.saturating_sub(shown);
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
    // Wrap math only — no syntect. Highlight does not change display width.
    estimate_child_terminal_rows(card, width) > budget
}

struct ChildRender {
    groups: Vec<ChildGroup>,
    /// Full terminal-row height of all children (painted + estimated tail).
    total_terminal_rows: usize,
}

/// Render fact/body groups. When `paint_budget` is `Some(n)`, only the first
/// `n` terminal rows are language-painted; the remainder is wrap-estimated so
/// collapse stays cheap and expand still shows an accurate "... N more" count.
fn render_child_groups(card: &ToolCard, width: usize, paint_budget: Option<usize>) -> ChildRender {
    let mut groups = Vec::new();
    let mut total_rows = 0usize;
    let mut paint_remaining = paint_budget.unwrap_or(usize::MAX);

    for fact in &card.facts {
        // Always mid trunk here; last-child └ / hang is rewritten after clip.
        let fact_lines = push_wrapped_tree_fact(fact_spans(fact), width);
        take_group(
            &mut groups,
            &mut total_rows,
            &mut paint_remaining,
            ChildGroup::TreeFact(fact_lines),
        );
    }

    match &card.body {
        ToolBody::None => {}
        ToolBody::Lines(body) => {
            let logical = tool_diff::logical_lines(body);
            let search_mode = card.match_pattern.is_some();
            let mut search = card.match_pattern.as_ref().map(|pattern| {
                SearchSyntax::new(crate::tui::syntax::MatchQuery::new(
                    pattern.clone(),
                    card.match_literal,
                    card.match_case_sensitive,
                ))
            });
            for line in &logical {
                if paint_remaining == 0 {
                    // Still count wrap height for "... N more" without paint.
                    total_rows = total_rows.saturating_add(if search_mode {
                        SearchSyntax::estimate_rows(line, width)
                    } else {
                        estimate_plain_body_rows(line, width)
                    });
                    continue;
                }
                let mut lines = Vec::new();
                if let Some(syntax) = search.as_mut() {
                    let _ = syntax.paint_line(line, width, &mut lines);
                } else {
                    push_body_line(&mut lines, line, width, Theme::text());
                }
                take_group(
                    &mut groups,
                    &mut total_rows,
                    &mut paint_remaining,
                    ChildGroup::Plain(lines),
                );
            }
        }
        ToolBody::Diff(rows) => {
            let gutter = tool_diff::gutter_width(rows);
            let fallback = rows
                .iter()
                .all(|row| row.kind != DiffRowKind::File)
                .then(|| tool_diff::single_file_path_from_header(card.family, &card.header))
                .flatten();
            let mut syntax = DiffSyntax::new(fallback);
            for row in rows {
                // Multi-file File headers are tree branches (path + structured
                // stats). Still feed DiffSyntax so language paint tracks path.
                if row.kind == DiffRowKind::File {
                    let _ = syntax.paint_row(row);
                    if paint_remaining == 0 {
                        total_rows =
                            total_rows.saturating_add(estimate_diff_row_rows(row, gutter, width));
                        continue;
                    }
                    let spans = file_section_spans(row);
                    take_group(
                        &mut groups,
                        &mut total_rows,
                        &mut paint_remaining,
                        ChildGroup::TreeFact(push_wrapped_tree_fact(spans, width)),
                    );
                    continue;
                }
                if paint_remaining == 0 {
                    total_rows =
                        total_rows.saturating_add(estimate_diff_row_rows(row, gutter, width));
                    continue;
                }
                let mut lines = Vec::new();
                push_diff_row(&mut lines, row, gutter, width, &mut syntax);
                take_group(
                    &mut groups,
                    &mut total_rows,
                    &mut paint_remaining,
                    ChildGroup::Plain(lines),
                );
            }
        }
    }

    ChildRender {
        groups,
        total_terminal_rows: total_rows,
    }
}

/// Push one child group, clipping to `paint_remaining` when set. Always adds
/// the full (pre-clip) height to `total_rows`.
fn take_group(
    groups: &mut Vec<ChildGroup>,
    total_rows: &mut usize,
    paint_remaining: &mut usize,
    group: ChildGroup,
) {
    let full = group.len().max(1);
    *total_rows = total_rows.saturating_add(full);
    if *paint_remaining == 0 {
        return;
    }
    if full <= *paint_remaining {
        *paint_remaining -= full;
        groups.push(group);
        return;
    }
    let mut clipped = group;
    clipped.truncate(*paint_remaining);
    *paint_remaining = 0;
    if !clipped.is_empty() {
        groups.push(clipped);
    }
}

/// Full child terminal-row estimate without language highlighting.
fn estimate_child_terminal_rows(card: &ToolCard, width: usize) -> usize {
    let mut total = 0usize;
    for fact in &card.facts {
        total = total.saturating_add(estimate_fact_rows(fact, width));
    }
    match &card.body {
        ToolBody::None => {}
        ToolBody::Lines(body) => {
            let logical = tool_diff::logical_lines(body);
            let search_mode = card.match_pattern.is_some();
            total = total.saturating_add(estimate_lines_rows(&logical, width, search_mode));
        }
        ToolBody::Diff(rows) => {
            let gutter = tool_diff::gutter_width(rows);
            total = total.saturating_add(estimate_diff_rows(rows, gutter, width));
        }
    }
    total
}

fn estimate_fact_rows(fact: &ToolFact, width: usize) -> usize {
    push_wrapped_tree_fact(fact_spans(fact), width).len().max(1)
}

fn estimate_plain_body_rows(line: &str, width: usize) -> usize {
    let prefix_width = display_width(CHILD_CONTENT_INDENT);
    let content_width = width.saturating_sub(prefix_width).max(1);
    wrap_line_hard(line, content_width).len().max(1)
}

fn estimate_lines_rows(lines: &[String], width: usize, search_mode: bool) -> usize {
    if search_mode {
        lines
            .iter()
            .map(|line| SearchSyntax::estimate_rows(line, width))
            .sum()
    } else {
        lines
            .iter()
            .map(|line| estimate_plain_body_rows(line, width))
            .sum()
    }
}

fn estimate_diff_rows(rows: &[DiffRow], gutter: usize, width: usize) -> usize {
    rows.iter()
        .map(|row| estimate_diff_row_rows(row, gutter, width))
        .sum()
}

fn estimate_diff_row_rows(row: &DiffRow, gutter: usize, width: usize) -> usize {
    if row.kind == DiffRowKind::File || row.kind == DiffRowKind::Meta {
        let prefix_width = display_width(CHILD_CONTENT_INDENT);
        let content_width = width.saturating_sub(prefix_width).max(1);
        return wrap_line_hard(&row.text, content_width).len().max(1);
    }
    let number = match (gutter, row.line) {
        (0, _) => 0,
        (_, Some(line)) => format!("{line} ").len().max(gutter + 1),
        (_, None) => gutter + 1,
    };
    let sign = 2usize; // "+ " / "- " / "  "
    let prefix_width = display_width(CHILD_CONTENT_INDENT) + number + sign;
    let content_width = width.saturating_sub(prefix_width).max(1);
    wrap_line_hard(&row.text, content_width).len().max(1)
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

/// Swap the leading content-indent columns for a continuing trunk stem (`  │ `).
///
/// Body/diff rows start with [`CHILD_CONTENT_INDENT`] (or a pure-space hang of
/// at least that width). When a later tree branch still follows, rewrite only
/// those leading columns so File headers read as one connected tree. Gutter
/// digits and other content after the indent keep their original style.
fn apply_trunk_stem(lines: &mut [Line<'static>]) {
    let indent_len = CHILD_CONTENT_INDENT.len();
    for line in lines {
        let Some(first) = line.spans.first() else {
            continue;
        };
        let text = first.content.as_ref();
        let rewrite_indent = text.starts_with(CHILD_CONTENT_INDENT)
            || (text.len() >= indent_len && text.bytes().all(|b| b == b' '));
        if !rewrite_indent {
            continue;
        }
        let rest = text[indent_len..].to_string();
        let rest_style = first.style;
        let mut spans = vec![Span::styled(TREE_WRAP_STEM.to_string(), Theme::tool_tree())];
        if !rest.is_empty() {
            spans.push(Span::styled(rest, rest_style));
        }
        spans.extend(line.spans.iter().skip(1).cloned());
        line.spans = spans;
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
        } => diff_stat_spans(*added, *removed, path.as_deref()),
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
/// Content lines may carry language-aware spans when a path is known.
fn push_diff_row(
    lines: &mut Vec<Line<'static>>,
    row: &DiffRow,
    gutter: usize,
    width: usize,
    syntax: &mut DiffSyntax,
) {
    // File rows are TreeFact groups in render_child_groups. Fallback keeps path
    // plain if a caller still routes a File row through this helper.
    let highlighted = syntax.paint_row(row);
    if row.kind == DiffRowKind::File {
        push_body_line(lines, &row.plain_text(), width, Theme::tool_path());
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
    // Sign cell is one character; a trailing space separates it from content
    // and sits in the row wash rather than the filled gutter.
    let sign = row.kind.sign();
    let sign_gap = " ";
    let prefix_width =
        display_width(CHILD_CONTENT_INDENT) + display_width(&number) + sign.len() + sign_gap.len();
    let content_width = width.saturating_sub(prefix_width).max(1);
    let text_style = Theme::tool_diff_text(row.kind);
    let sign_style = Theme::tool_diff_sign(row.kind);
    let row_wash = Theme::tool_diff_row(row.kind);
    let number_style = match row_wash {
        Some(wash) => Theme::tool_diff_gutter().patch(wash),
        None => Theme::tool_diff_gutter(),
    };
    let gap_style = match row_wash {
        Some(wash) => text_style.patch(wash),
        None => text_style,
    };
    let pad_style = row_wash.unwrap_or_else(Theme::text);

    let mut content_spans = match highlighted {
        Some(segments) => spans_from_segments_with_matches(&segments, text_style, &[]),
        None => vec![Span::styled(row.text.clone(), text_style)],
    };
    if let Some(wash) = row_wash {
        for span in &mut content_spans {
            span.style = span.style.patch(wash);
        }
    }

    let wrapped = hard_wrap_styled_spans(&row.text, &content_spans, content_width, text_style);
    let indent_width = display_width(CHILD_CONTENT_INDENT);
    for (index, chunk) in wrapped.into_iter().enumerate() {
        let mut spans = if index == 0 {
            vec![
                Span::styled(CHILD_CONTENT_INDENT.to_string(), Theme::tool_tree()),
                Span::styled(number.clone(), number_style),
                Span::styled(sign.to_string(), sign_style),
                Span::styled(sign_gap.to_string(), gap_style),
            ]
        } else {
            // Continuations keep tree indent clear; wash covers number+sign columns.
            let mut cont = vec![Span::styled(
                CHILD_CONTENT_INDENT.to_string(),
                Theme::tool_tree(),
            )];
            let rest = prefix_width.saturating_sub(indent_width);
            if rest > 0 {
                cont.push(Span::styled(
                    " ".repeat(rest),
                    match row_wash {
                        Some(wash) => Theme::tool_tree().patch(wash),
                        None => Theme::tool_tree(),
                    },
                ));
            }
            cont
        };
        spans.extend(chunk);
        lines.push(pad_spans_line_with(spans, width, pad_style));
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

/// File section header spans from structured path + optional stats.
fn file_section_spans(row: &DiffRow) -> Vec<Span<'static>> {
    match row.stats {
        Some((added, removed)) => diff_stat_spans(added, removed, Some(row.text.as_str())),
        None => vec![Span::styled(row.text.clone(), Theme::tool_path())],
    }
}

/// Shared colored spans for DiffStat facts and multi-file File section headers.
fn diff_stat_spans(added: u64, removed: u64, path: Option<&str>) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(format!("+{added}"), Theme::tool_stat_add()),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Theme::tool_stat_del()),
        Span::styled(" lines", Theme::tool_meta()),
    ];
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        spans.push(Span::styled(" | ", Theme::tool_meta()));
        spans.push(Span::styled(path.to_string(), Theme::tool_path()));
    }
    spans
}

fn pad_spans_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    pad_spans_line_with(spans, width, Theme::text())
}

fn pad_spans_line_with(
    mut spans: Vec<Span<'static>>,
    width: usize,
    pad_style: Style,
) -> Line<'static> {
    let used = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), pad_style));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "tool_card_render_tests.rs"]
mod tests;
