//! Borderless message cards. Routing and task selection belong to the presenter.

use ratatui::text::{Line, Span};

use crate::app::message_card::{MessageCard, MessageDelivery};

use super::{
    render::{push_wrapped_text, truncate_one_line, LineFill},
    theme::Theme,
};

pub(super) fn message_card_lines(
    message: &MessageCard,
    width: usize,
    preview_lines: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(2).max(1);
    let mut lines = vec![Line::from(vec![
        Span::styled("↳ ", Theme::dim()),
        Span::styled(
            truncate_one_line(&safe_message_text(&message.title), content_width),
            Theme::tool_primary(),
        ),
    ])];
    let delivery = match message.delivery {
        MessageDelivery::Queued => "queued",
    };
    let routing = format!("{} → {} · {delivery}", message.sender, message.recipient);
    push_indented(&mut lines, &routing, content_width, Theme::dim());

    let mut body = Vec::new();
    push_indented(&mut body, &message.body, content_width, Theme::text());
    let budget = preview_lines.max(1);
    let hidden = body.len().saturating_sub(budget);
    if !expanded {
        body.truncate(budget);
    }
    lines.extend(body);
    if expanded {
        push_indented(
            &mut lines,
            &format!("task: {}", message.title),
            content_width,
            Theme::dim(),
        );
        for detail in &message.details {
            push_indented(&mut lines, detail, content_width, Theme::dim());
        }
    }
    let hint = if expanded {
        "Ctrl+O collapse".to_string()
    } else if hidden > 0 {
        format!("… {hidden} more lines · Ctrl+O expand")
    } else {
        "Ctrl+O details".to_string()
    };
    push_indented(&mut lines, &hint, content_width, Theme::dim());
    lines
}

fn push_indented(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: ratatui::style::Style,
) {
    let start = lines.len();
    push_wrapped_text(
        lines,
        &safe_message_text(text),
        width,
        style,
        LineFill::Natural,
    );
    for line in &mut lines[start..] {
        line.spans.insert(0, Span::raw("  "));
    }
}

/// Keep message line breaks and ordinary Unicode, but make terminal and bidi
/// controls visible so they cannot hide content or alter its apparent direction.
fn safe_message_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for ch in text.chars() {
        if (ch.is_control() && ch != '\n')
            || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            safe.extend(ch.escape_default());
        } else {
            safe.push(ch);
        }
    }
    safe
}

#[cfg(test)]
#[path = "message_card_render_tests.rs"]
mod tests;
