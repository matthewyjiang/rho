//! Notification cards. Routing and task selection belong to the presenter.

use ratatui::text::{Line, Span};

use crate::presentation::{
    NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
};

use super::{
    markdown::{push_wrapped_markdown_without_copy_button_from_fence_state, CodeFenceState},
    render::{push_wrapped_text, truncate_one_line, LineFill},
    theme::Theme,
    tool_card_render::CardSections,
};

pub(super) fn notification_card_sections(
    message: &NotificationCard,
    width: usize,
    preview_lines: usize,
    expanded: bool,
) -> CardSections {
    let received = matches!(message.delivery, NotificationDelivery::Received);
    let rail_width = if received { 2 } else { 0 };
    let content_width = width.saturating_sub(rail_width + 2).max(1);
    let tone = match message.tone {
        NotificationTone::Neutral => Theme::tool_primary(),
        NotificationTone::Accent => Theme::accent(),
        NotificationTone::Success => Theme::success(),
        NotificationTone::Warning => Theme::warning(),
        NotificationTone::Error => Theme::error(),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(if received { "↰ " } else { "↳ " }, tone),
        Span::styled(
            truncate_one_line(&safe_message_text(&message.title), content_width),
            tone,
        ),
    ])];
    let subtitle = message.subtitle.clone().unwrap_or_else(|| {
        let delivery = match message.delivery {
            NotificationDelivery::Queued => "queued",
            NotificationDelivery::Received => "received",
        };
        let mut routing = format!("{} → {} · {delivery}", message.sender, message.recipient);
        if let Some(reference) = &message.reference {
            routing.push_str(&format!(" · {reference}"));
        }
        routing
    });
    push_indented(&mut lines, &subtitle, content_width, Theme::dim());

    let mut body = Vec::new();
    // Empty bodies (e.g. a silent process) would otherwise render a blank row.
    if !message.body.trim().is_empty() {
        push_wrapped_markdown_without_copy_button_from_fence_state(
            &mut body,
            &safe_message_text(&message.body),
            content_width,
            &mut CodeFenceState::default(),
        );
    }
    for line in &mut body {
        line.spans.insert(0, Span::raw("  "));
    }
    let budget = preview_lines.max(1);
    let hidden = body.len().saturating_sub(budget);
    let show_full_body = expanded || matches!(message.preview, NotificationPreview::Full);
    if !show_full_body {
        body.truncate(budget);
    }
    if expanded {
        for detail in &message.details {
            push_indented(&mut body, detail, content_width, Theme::dim());
        }
    }
    // Keybinds live in the help window; the card only says what is hidden.
    if hidden > 0 && !show_full_body {
        push_indented(
            &mut body,
            &format!("… {hidden} more lines"),
            content_width,
            Theme::dim(),
        );
    }
    if received {
        for line in lines.iter_mut().chain(body.iter_mut()) {
            line.spans.insert(0, Span::styled("│ ", tone));
        }
    }
    CardSections {
        header: lines,
        facts: Vec::new(),
        body,
        last_fact_is_end: false,
    }
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
#[path = "notification_card_render_tests.rs"]
mod tests;
