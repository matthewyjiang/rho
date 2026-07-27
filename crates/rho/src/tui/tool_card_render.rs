//! Multi-span Call + Children rendering for structured tool cards.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolHeader, ToolStatus};
use unicode_width::UnicodeWidthStr;

use super::{
    feed_image::reserve_optional_image_rows,
    render::{
        display_width, pad_entry_line, padded_inner_width, push_hard_wrapped_text,
        push_wrapped_text, styled_blank_line, wrap_line_hard, LineFill,
    },
    theme::Theme,
    tool_diff, ToolEntry, ToolEntryState,
};

const TREE_INDENT: &str = "  ";
const TREE_BRANCH_MID: &str = "├ ";
const TREE_BRANCH_END: &str = "└ ";
const TREE_CONTINUE: &str = "  ";
/// Content column after `  ├ ` / `  └ `.
const CHILD_CONTENT_INDENT: &str = "    ";

pub(super) fn tool_entry_lines(
    tool: &ToolEntry,
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    let inner_width = padded_inner_width(width);
    let mut lines = Vec::new();
    if let Some(card) = tool.card.as_ref() {
        push_tool_card(
            &mut lines,
            card,
            tool.state,
            inner_width,
            max_tool_output_lines,
            tool.expanded,
        );
    } else {
        push_legacy_tool_block(
            &mut lines,
            &tool.display_lines,
            tool.state,
            inner_width,
            max_tool_output_lines,
            tool.expanded,
        );
    }
    reserve_optional_image_rows(&mut lines, tool.image.as_ref(), width);
    let padding_style = Theme::tool_card_padding();
    let mut padded = Vec::with_capacity(lines.len() + 2);
    padded.push(styled_blank_line(width, padding_style));
    padded.extend(lines.into_iter().map(pad_entry_line));
    padded.push(styled_blank_line(width, padding_style));
    padded
}

pub(super) fn push_tool_card(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCard,
    state: ToolEntryState,
    width: usize,
    max_tool_output_lines: usize,
    expanded: bool,
) {
    let status = status_for_state(card, state);
    push_header_line(lines, card, status, width);

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

fn status_for_state(card: &ToolCard, state: ToolEntryState) -> ToolStatus {
    match state {
        ToolEntryState::Running => ToolStatus::Running,
        ToolEntryState::Finished { ok, .. } => {
            if matches!(card.status, ToolStatus::Interrupted | ToolStatus::Blocked) {
                card.status
            } else {
                ToolStatus::from_finished(ok)
            }
        }
    }
}

fn push_header_line(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCard,
    status: ToolStatus,
    width: usize,
) {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!("{} ", status.marker()),
        Theme::tool_marker(status),
    ));
    match &card.header {
        ToolHeader::Call { verb, primary } => {
            spans.push(Span::styled(verb.clone(), Theme::tool_verb(card.family)));
            if let Some(primary) = primary.as_ref().filter(|primary| !primary.is_empty()) {
                spans.push(Span::styled("(", Theme::tool_primary(card.family)));
                spans.push(Span::styled(
                    primary.clone(),
                    Theme::tool_primary(card.family),
                ));
                spans.push(Span::styled(")", Theme::tool_primary(card.family)));
            }
        }
        ToolHeader::Shell { prompt, command } => {
            spans.push(Span::styled(prompt.clone(), Theme::tool_verb(card.family)));
            if let Some(command) = command.as_ref().filter(|command| !command.is_empty()) {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    command.clone(),
                    Theme::tool_primary(card.family),
                ));
            }
        }
        ToolHeader::StatusFirst { identity, detail } => {
            spans.push(Span::styled(
                identity.clone(),
                Theme::tool_verb(card.family),
            ));
            if !detail.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(detail.clone(), Theme::text()));
            }
        }
    }
    lines.push(pad_spans_line(spans, width));
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
    let content_spans = fact_spans(fact);
    let content_text: String = content_spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let wrapped = wrap_line_hard(&content_text, content_width);
    if wrapped.is_empty() {
        lines.push(pad_spans_line(
            vec![
                Span::styled(prefix, Theme::tool_tree()),
                Span::styled(String::new(), Theme::text()),
            ],
            width,
        ));
        return;
    }

    // First line uses tree branch; continuations align to the content column.
    let first = &wrapped[0];
    let first_spans = truncate_spans_to_text(&content_spans, first);
    let mut first_line = vec![Span::styled(prefix, Theme::tool_tree())];
    first_line.extend(first_spans);
    lines.push(pad_spans_line(first_line, width));

    for chunk in wrapped.iter().skip(1) {
        let cont_spans = truncate_spans_to_text(&content_spans, chunk);
        // Re-slice spans relative to chunk is hard; restyle plain chunk with meta.
        let mut cont = vec![Span::styled(
            format!("{TREE_INDENT}{TREE_CONTINUE}"),
            Theme::tool_tree(),
        )];
        if cont_spans.is_empty() {
            cont.push(Span::styled(chunk.clone(), Theme::tool_meta()));
        } else {
            cont.extend(cont_spans);
        }
        lines.push(pad_spans_line(cont, width));
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

fn push_legacy_tool_block(
    lines: &mut Vec<Line<'static>>,
    display_lines: &[String],
    state: ToolEntryState,
    width: usize,
    max_tool_output_lines: usize,
    expanded: bool,
) {
    use rho_tools::tool::ToolDisplayStyle;
    let style = match state {
        ToolEntryState::Running => Theme::user_message(),
        ToolEntryState::Finished { ok, display_style } => match display_style {
            ToolDisplayStyle::DefaultTool => Theme::tool_default().for_result(ok),
            ToolDisplayStyle::FileOrCommand | ToolDisplayStyle::FileDiff => {
                Theme::tool_file_or_command().for_result(ok)
            }
            ToolDisplayStyle::Skill => Theme::tool_skill().for_result(ok),
            ToolDisplayStyle::Web => Theme::tool_web().for_result(ok),
            ToolDisplayStyle::Questionnaire => Theme::tool_questionnaire().for_result(ok),
        },
    };
    let color_diff = matches!(
        state,
        ToolEntryState::Finished {
            display_style: ToolDisplayStyle::FileDiff,
            ..
        }
    );
    let logical_lines = tool_diff::logical_lines(display_lines);
    let max_tool_output_lines = max_tool_output_lines.max(1);
    let truncated = logical_lines.len() > max_tool_output_lines;
    let visible_count = if truncated && !expanded {
        max_tool_output_lines
    } else {
        logical_lines.len()
    };

    for line in logical_lines.iter().take(visible_count) {
        let line_style = if color_diff {
            tool_diff::line_style(line, style)
        } else {
            style
        };
        push_hard_wrapped_text(lines, line, width, line_style, LineFill::PadToWidth);
    }

    if truncated {
        let prompt = if expanded {
            "ctrl+o to collapse".to_string()
        } else {
            format!(
                "... {} more lines, ctrl+o to expand",
                logical_lines.len() - visible_count
            )
        };
        push_wrapped_text(lines, &prompt, width, style, LineFill::PadToWidth);
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

/// Best-effort: when wrapping loses multi-span alignment, fall back to plain text.
fn truncate_spans_to_text(spans: &[Span<'static>], text: &str) -> Vec<Span<'static>> {
    let full: String = spans.iter().map(|span| span.content.as_ref()).collect();
    if full == text {
        return spans.to_vec();
    }
    if full.starts_with(text) {
        // Prefix of the original span stream.
        let mut remaining = text.chars().count();
        let mut out = Vec::new();
        for span in spans {
            if remaining == 0 {
                break;
            }
            let content = span.content.as_ref();
            let chars = content.chars().count();
            if chars <= remaining {
                out.push(span.clone());
                remaining -= chars;
            } else {
                let clipped: String = content.chars().take(remaining).collect();
                out.push(Span::styled(clipped, span.style));
                remaining = 0;
            }
        }
        return out;
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rho_tools::tool_card::{ToolFamily, ToolHeader, ToolStatus};

    use super::*;
    use crate::tui::ToolEntryState;

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
        push_tool_card(
            &mut lines,
            &card,
            ToolEntryState::Finished {
                ok: true,
                display_style: rho_tools::tool::ToolDisplayStyle::file_diff(),
            },
            80,
            4,
            /*expanded*/ false,
        );
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
        push_tool_card(
            &mut lines,
            &card,
            ToolEntryState::Finished {
                ok: true,
                display_style: rho_tools::tool::ToolDisplayStyle::file_or_command(),
            },
            80,
            1,
            /*expanded*/ false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(rendered[0].starts_with("✓ $ cargo test"));
        assert!(rendered.iter().any(|line| line.contains("timeout 30s")));
        assert!(rendered.iter().any(|line| line.contains("exit 0")));
        assert!(rendered.iter().any(|line| line.contains("more lines")));
    }
}
