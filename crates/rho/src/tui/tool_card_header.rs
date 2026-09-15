//! Header dialects share the card's styled wrapping and continuation geometry.

use ratatui::text::{Line, Span};
use rho_tools::tool_card::{ToolCard, ToolHeader, ToolStatus};

use super::{header_wrap_continuation_prefix, pad_spans_line, push_wrapped_prefixed};
use crate::tui::{syntax::highlight_source_spans, theme::Theme};

pub(super) fn push_header_line(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCard,
    status: ToolStatus,
    width: usize,
) {
    // Keep the status marker on the first row and hang wrapped arguments below
    // their prefix so long streamed arguments remain visible.
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
                    let wrappable = shell_command_spans(prompt, command);
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

/// Highlight a shell header while preserving its exact text for styled wrapping.
fn shell_command_spans(prompt: &str, command: &str) -> Vec<Span<'static>> {
    let language = if prompt.eq_ignore_ascii_case("PS") {
        "powershell"
    } else {
        "bash"
    };
    highlight_source_spans(language, command, Theme::tool_primary())
}
