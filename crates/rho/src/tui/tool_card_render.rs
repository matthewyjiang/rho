//! Multi-span Call + Children rendering for structured tool cards.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolHeader, ToolStatus};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    feed_image::reserve_optional_image_rows,
    render::{
        display_width, pad_entry_line, padded_inner_width, push_wrapped_text, styled_blank_line,
        wrap_line_at_whitespace_ranges, wrap_line_hard, LineFill,
    },
    theme::Theme,
    tool_diff, ToolEntry,
};

const TREE_INDENT: &str = "  ";
const TREE_BRANCH_MID: &str = "├ ";
const TREE_BRANCH_END: &str = "└ ";
const TREE_CONTINUE: &str = "  ";
/// Vertical stem on wrapped header rows; same box-drawing family as ├ / └.
const HEADER_WRAP_STEM: &str = "  │ ";
/// Content column after `  ├ ` / `  └ `.
const CHILD_CONTENT_INDENT: &str = "    ";

pub(super) fn tool_entry_lines(
    tool: &ToolEntry,
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    let inner_width = padded_inner_width(width);
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
    padded.extend(lines.into_iter().map(pad_entry_line));
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

    let fact_count = card.facts.len();
    let body_lines = body_logical_lines(&card.body);
    let show_body =
        !body_lines.is_empty() && (expanded || !matches!(card.body, ToolBody::DiffLines(_)));
    // Diff bodies stay collapsed unless expanded; other bodies use the line budget.
    let max_body = max_tool_output_lines.max(1);
    let truncated = show_body && body_lines.len() > max_body && !expanded;
    let visible_body = if !show_body {
        0
    } else if truncated {
        max_body
    } else {
        body_lines.len()
    };

    for (index, fact) in card.facts.iter().enumerate() {
        let is_last = index + 1 == fact_count && visible_body == 0 && !truncated;
        push_fact_line(lines, fact, is_last, width);
    }

    if visible_body > 0 {
        let color_diff = card.body.is_diff();
        for line in body_lines.iter().take(visible_body) {
            let style = if color_diff {
                tool_diff::line_style(line, Theme::text())
            } else {
                Theme::text()
            };
            push_body_line(lines, line, width, style);
        }
    }

    if truncated || (show_body && body_lines.len() > max_body && expanded) {
        let prompt = if expanded {
            "ctrl+o to collapse".to_string()
        } else {
            format!(
                "... {} more lines, ctrl+o to expand",
                body_lines.len().saturating_sub(visible_body)
            )
        };
        push_wrapped_text(lines, &prompt, width, Theme::dim(), LineFill::PadToWidth);
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
                    prefix.push(Span::styled("(", Theme::tool_primary(card.family)));
                    let wrappable = vec![
                        Span::styled(primary.clone(), Theme::tool_primary(card.family)),
                        Span::styled(")", Theme::tool_primary(card.family)),
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
                    let wrappable = vec![Span::styled(
                        command.clone(),
                        Theme::tool_primary(card.family),
                    )];
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
/// Continuations draw a tree-column `|` elbow, then pad to the primary hang so
/// children (`├` / `└`) still read as a connected trunk under the call.
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

    let ranges = wrap_line_at_whitespace_ranges(&text, content_width);
    for (index, range) in ranges.into_iter().enumerate() {
        let mut start = range.start;
        let end = range.end;
        if index > 0 {
            // Keep hang indent stable when a wrap boundary leaves leading spaces.
            while start < end {
                let ch = text[start..].chars().next().expect("start < end");
                if !ch.is_whitespace() {
                    break;
                }
                start += ch.len_utf8();
            }
            if start >= end {
                continue;
            }
        }
        let chunk_spans = slice_spans_by_bytes(&wrappable, start, end);
        let mut row = if index == 0 {
            prefix.clone()
        } else {
            header_wrap_continuation_prefix(hang)
        };
        row.extend(chunk_spans);
        lines.push(pad_spans_line(row, width));
    }
}

/// `  │ ` in the child elbow column, then spaces out to the primary hang.
fn header_wrap_continuation_prefix(hang: usize) -> Vec<Span<'static>> {
    let stem_width = display_width(HEADER_WRAP_STEM);
    let mut spans = vec![Span::styled(
        HEADER_WRAP_STEM.to_string(),
        Theme::tool_tree(),
    )];
    if hang > stem_width {
        spans.push(Span::styled(" ".repeat(hang - stem_width), Theme::text()));
    }
    spans
}

fn spans_display_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn slice_spans_by_bytes(spans: &[Span<'static>], start: usize, end: usize) -> Vec<Span<'static>> {
    if start >= end {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    for span in spans {
        let content = span.content.as_ref();
        let span_start = offset;
        let span_end = offset + content.len();
        offset = span_end;
        if span_end <= start || span_start >= end {
            continue;
        }
        let from = start.saturating_sub(span_start);
        let to = (end - span_start).min(content.len());
        if from >= to {
            continue;
        }
        // Ranges come from the concatenated UTF-8 text, so byte edges are char edges.
        out.push(Span::styled(content[from..to].to_string(), span.style));
    }
    out
}

fn push_fact_line(lines: &mut Vec<Line<'static>>, fact: &ToolFact, is_last: bool, width: usize) {
    let branch = if is_last {
        TREE_BRANCH_END
    } else {
        TREE_BRANCH_MID
    };
    let prefix = format!("{TREE_INDENT}{branch}");
    let prefix_width = display_width(&prefix);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let wrapped = wrap_spans_hard(&fact_spans(fact), content_width);

    // First line uses tree branch; continuations align to the content column.
    let mut first_line = vec![Span::styled(prefix, Theme::tool_tree())];
    first_line.extend(wrapped[0].clone());
    lines.push(pad_spans_line(first_line, width));

    for row in wrapped.iter().skip(1) {
        let mut continuation = vec![Span::styled(
            format!("{TREE_INDENT}{TREE_CONTINUE}"),
            Theme::tool_tree(),
        )];
        continuation.extend(row.clone());
        lines.push(pad_spans_line(continuation, width));
    }
}

fn wrap_spans_hard(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut used = 0;

    for span in spans {
        let mut chunk = String::new();
        for character in span.content.chars() {
            if character == '\n' {
                push_span_chunk(&mut row, &mut chunk, span.style);
                rows.push(std::mem::take(&mut row));
                used = 0;
                continue;
            }
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if used > 0 && used + character_width > width {
                push_span_chunk(&mut row, &mut chunk, span.style);
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            chunk.push(character);
            used += character_width;
            if used >= width {
                push_span_chunk(&mut row, &mut chunk, span.style);
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
        }
        push_span_chunk(&mut row, &mut chunk, span.style);
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

fn push_span_chunk(row: &mut Vec<Span<'static>>, chunk: &mut String, style: Style) {
    if !chunk.is_empty() {
        row.push(Span::styled(std::mem::take(chunk), style));
    }
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
            let ok = *code == 0;
            let mut spans = vec![Span::styled(format!("exit {code}"), Theme::tool_exit(ok))];
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

fn body_logical_lines(body: &ToolBody) -> Vec<String> {
    match body {
        ToolBody::None => Vec::new(),
        ToolBody::Lines(lines) | ToolBody::DiffLines(lines) => tool_diff::logical_lines(lines),
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
mod tests {
    use pretty_assertions::assert_eq;
    use rho_tools::tool_card::{ToolBody, ToolFact, ToolFamily, ToolHeader, ToolStatus};

    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn renders_edit_card_with_diff_stat_child() {
        let card = ToolCard::new(
            ToolStatus::Ok,
            ToolFamily::FileCommand,
            ToolHeader::call("edit_file", Some("theme.rs".into())),
        )
        .with_facts(vec![ToolFact::DiffStat {
            added: 54,
            removed: 2,
            path: Some("theme.rs".into()),
        }])
        .with_body(ToolBody::DiffLines(vec!["-old".into(), "+new".into()]));
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, 80, 4, /*expanded*/ false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(rendered[0], "✓ edit_file(theme.rs)");
        assert!(rendered[1].contains("└"));
        assert!(rendered[1].contains("+54"));
        assert!(rendered[1].contains("-2"));
        assert_eq!(rendered.len(), 2, "collapsed edit hides diff body");
    }

    #[test]
    fn header_and_facts_survive_tiny_body_budget() {
        let card = ToolCard::new(
            ToolStatus::Ok,
            ToolFamily::FileCommand,
            ToolHeader::shell("$", Some("cargo test".into())),
        )
        .with_facts(vec![
            ToolFact::Meta {
                text: "timeout 30s".into(),
            },
            ToolFact::Exit {
                code: 0,
                duration_ms: Some(100),
            },
        ])
        .with_body(ToolBody::Lines(vec![
            "line1".into(),
            "line2".into(),
            "line3".into(),
        ]));
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, 80, 1, /*expanded*/ false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(rendered[0].starts_with("✓ $ cargo test"));
        assert!(rendered.iter().any(|line| line.contains("timeout 30s")));
        assert!(rendered.iter().any(|line| line.contains("exit 0")));
        assert!(rendered.iter().any(|line| line.contains("more lines")));
    }

    #[test]
    fn tool_entry_lines_use_trailing_blank_only() {
        let card = ToolCard::new(
            ToolStatus::Ok,
            ToolFamily::FileCommand,
            ToolHeader::call("read_file", Some("main.rs".into())),
        );
        let tool = crate::tui::ToolEntry {
            card,
            expanded: false,
            image: None,
        };
        let lines = tool_entry_lines(&tool, 40, 4);
        assert!(
            line_text(&lines[0]).contains("✓ read_file(main.rs)"),
            "unexpected header: {}",
            line_text(&lines[0])
        );
        assert!(
            line_text(lines.last().expect("card lines")).is_empty(),
            "expected a single trailing spacer"
        );
        assert!(
            lines.len() >= 2 && !line_text(&lines[lines.len() - 2]).is_empty(),
            "tool cards should not keep a leading spacer blank"
        );
    }

    #[test]
    fn long_shell_header_wraps_command_under_prompt() {
        let command = "cargo test -p rho-coding-agent --lib interactive_presenter -- --nocapture";
        let card = ToolCard::new(
            ToolStatus::Running,
            ToolFamily::FileCommand,
            ToolHeader::shell("$", Some(command.into())),
        )
        .with_facts(vec![ToolFact::Meta {
            text: "timeout 30s".into(),
        }]);
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, 40, 10, /*expanded*/ false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(
            rendered.len() >= 3,
            "long command should wrap before facts: {rendered:?}"
        );
        assert!(
            rendered[0].starts_with("● $ "),
            "marker+prompt stay on first header row: {rendered:?}"
        );
        assert!(
            !rendered[0].contains('├') && !rendered[0].contains('└'),
            "header must not use tree glyphs: {rendered:?}"
        );
        // Continuation uses a tree-column stem, then hangs under the primary.
        let cont = &rendered[1];
        assert!(
            cont.contains('│'),
            "header continuation should draw a │ stem: {rendered:?}"
        );
        assert!(
            !cont.contains('├') && !cont.contains('└'),
            "header continuation must not use child branch glyphs: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("timeout 30s")
                    && (line.contains('├') || line.contains('└'))),
            "facts keep tree structure after wrapped header: {rendered:?}"
        );
        let joined: String = rendered.iter().map(|line| line.trim()).collect();
        assert!(
            joined.contains("interactive_presenter") && joined.contains("nocapture"),
            "full command remains visible after wrap: {rendered:?}"
        );
    }

    #[test]
    fn long_call_header_wraps_primary_inside_parens() {
        let card = ToolCard::new(
            ToolStatus::Ok,
            ToolFamily::FileCommand,
            ToolHeader::call(
                "read_file",
                Some("crates/rho/src/tui/tool_card_render.rs".into()),
            ),
        );
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, 28, 4, /*expanded*/ false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(
            rendered[0].starts_with("✓ read_file("),
            "verb and open paren stay on first row: {rendered:?}"
        );
        assert!(rendered.len() >= 2, "long path should wrap: {rendered:?}");
        assert!(
            rendered.iter().skip(1).all(|line| line.contains('│')),
            "wrapped call primary should use │ stems: {rendered:?}"
        );
        let path_text: String = rendered
            .iter()
            .map(|line| line.trim().trim_start_matches('│').trim().to_string())
            .collect();
        assert!(
            path_text.contains("tool_card_render.rs") && path_text.contains(')'),
            "path and closing paren remain visible: {rendered:?}"
        );
    }

    #[test]
    fn running_shell_card_renders_streamed_stdout_body() {
        let card = ToolCard::new(
            ToolStatus::Running,
            ToolFamily::FileCommand,
            ToolHeader::shell("$", Some("cargo test".into())),
        )
        .with_facts(vec![
            ToolFact::Meta {
                text: "timeout 30s".into(),
            },
            ToolFact::Meta {
                text: "running".into(),
            },
        ])
        .with_body(ToolBody::Lines(vec![
            "compiling rho".into(),
            "running 12 tests".into(),
        ]));
        let tool = crate::tui::ToolEntry {
            card,
            expanded: false,
            image: None,
        };
        let rendered: Vec<String> = tool_entry_lines(&tool, 60, 10)
            .iter()
            .map(line_text)
            .collect();
        assert!(
            rendered.iter().any(|line| line.contains("compiling rho")),
            "streamed stdout missing from rendered card: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("running 12 tests")),
            "streamed stdout missing from rendered card: {rendered:?}"
        );
    }

    #[test]
    fn finished_background_agent_keeps_running_marker() {
        let card = ToolCard::new(
            ToolStatus::Running,
            ToolFamily::Agent,
            ToolHeader::status_first("worker", "running in background"),
        )
        .with_facts(vec![ToolFact::Text {
            text: "fixture stream".into(),
        }])
        .with_body(ToolBody::Lines(vec!["abc123 · rho attach abc123".into()]));
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, 80, 10, /*expanded*/ false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(
            rendered[0].starts_with("● worker  running in background"),
            "background spawn must keep the running marker after tool finish: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("fixture stream")),
            "background task text missing: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("abc123 · rho attach abc123")),
            "background run meta missing: {rendered:?}"
        );
    }
}
