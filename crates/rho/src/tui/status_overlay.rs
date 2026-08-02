//! Tiny disappearing feedback toast for action and mode status.

use std::time::{Duration, Instant};

use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    widgets::{Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::{render::truncate_one_line, theme::Theme};

/// Match the copy toast so status feedback does not linger longer than other
/// corner notices.
const STATUS_OVERLAY_DURATION: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusTone {
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusOverlay {
    message: String,
    tone: StatusTone,
    visible_until: Instant,
}

impl StatusOverlay {
    pub(super) fn new(message: impl Into<String>, now: Instant) -> Self {
        let message = message.into();
        let tone = tone_for_message(&message);
        Self {
            message,
            tone,
            visible_until: now + STATUS_OVERLAY_DURATION,
        }
    }

    pub(super) fn message(&self) -> &str {
        &self.message
    }

    pub(super) fn tone(&self) -> StatusTone {
        self.tone
    }

    pub(super) fn visible_until(&self) -> Instant {
        self.visible_until
    }

    pub(super) fn is_visible(&self, now: Instant) -> bool {
        now < self.visible_until
    }
}

/// Idle mode labels stay out of the toast so they do not flash after every turn
/// or leave box-drawing chrome in the corner during resize checks.
pub(super) fn should_toast(message: &str) -> bool {
    !matches!(
        message,
        "ready" | "running" | "config" | "skills" | "workflows"
    )
}

/// Classify toast color from message text: error, success, else warning/busy.
pub(super) fn tone_for_message(message: &str) -> StatusTone {
    let message = message.to_ascii_lowercase();
    if contains_any(
        &message,
        &[
            "fail",
            "error",
            "rejected",
            "unavailable",
            "invalid",
            "conflict",
            "could not",
            "cannot",
            "incomplete",
        ],
    ) {
        return StatusTone::Error;
    }
    if contains_any(
        &message,
        &[
            "saved",
            "complete",
            "attached",
            "loaded",
            "cleared",
            "done",
            "inserted",
            "updated",
            "renamed",
            "exported",
            "deleted",
            "success",
            "unchanged",
        ],
    ) {
        return StatusTone::Success;
    }
    StatusTone::Warning
}

fn contains_any(message: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| message.contains(needle))
}

fn tone_style(tone: StatusTone) -> Style {
    match tone {
        StatusTone::Success => Theme::success(),
        StatusTone::Warning => Theme::warning(),
        StatusTone::Error => Theme::error(),
    }
}

/// Draw a one-line top-right toast, stacked under `top_offset` rows (copy notice).
///
/// No box borders: mermaid and other art detectors treat `┌` as diagram chrome.
pub(super) fn render_status_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    overlay: &StatusOverlay,
    now: Instant,
    top_offset: u16,
) {
    if !overlay.is_visible(now) || area.width == 0 || area.height == 0 {
        return;
    }
    if top_offset >= area.height {
        return;
    }

    let max_width = area.width as usize;
    let text = truncate_one_line(overlay.message(), max_width.saturating_sub(2).max(1));
    let popup_width = UnicodeWidthStr::width(text.as_str())
        .saturating_add(2)
        .min(max_width)
        .max(1) as u16;
    let popup = Rect::new(
        area.x
            .saturating_add(area.width.saturating_sub(popup_width)),
        area.y.saturating_add(top_offset),
        popup_width,
        1,
    );

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(format!(" {text} "))
            .alignment(Alignment::Right)
            .style(tone_style(overlay.tone())),
        popup,
    );
}

#[cfg(test)]
#[path = "status_overlay_tests.rs"]
mod tests;
