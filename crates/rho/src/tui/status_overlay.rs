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
    pub(super) fn new(message: impl Into<String>, tone: StatusTone, now: Instant) -> Self {
        Self {
            message: message.into(),
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

/// Decide whether `set_status` should open the corner toast.
///
/// Keep outcomes, errors, and actionable feedback. Drop:
/// - idle / mode labels already mirrored by the composer or activity rail
/// - routine progress that repeats every step or tool phase
/// - picker and editor chrome whose title is already on screen
pub(super) fn should_toast(message: &str) -> bool {
    !is_routine_status(message)
}

fn is_routine_status(message: &str) -> bool {
    let message = message.trim();
    if message.is_empty() {
        return true;
    }

    // Exact mode labels and progress already covered by activity / composer UI.
    if matches!(
        message,
        "ready"
            | "running"
            | "config"
            | "skills"
            | "workflows"
            | "error"
            | "approval requested"
            | "compacting context"
            | "retrying provider response"
            | "rate limited · retrying"
            | "evaluating goal"
            | "goal retrying"
            | "loading models"
            | "refreshing model list"
            | "checking OAuth usage limits"
            | "checking provider connections"
            | "fetching latest changelog"
            | "waiting for delegated agents"
            | "waiting for approval"
            | "waiting for your answers"
            | "keyboard shortcuts"
            | "runtime info"
            | "doctor diagnostics"
            | "web search config"
            | "changelog usage"
    ) {
        return true;
    }

    let lower = message.to_ascii_lowercase();

    // Prefixes for step/tool progress and open picker/editor chrome.
    if lower.starts_with("running step ")
        || lower.starts_with("running ")
        || lower.starts_with("select ")
        || lower.starts_with("edit ")
        || lower.starts_with("confirm ")
        || lower.starts_with("choose ")
        || lower.starts_with("extracting ")
        || lower.starts_with("checking ")
        || lower.starts_with("fetching ")
        || lower.starts_with("loading ")
        || lower.starts_with("refreshing ")
        || lower.starts_with("waiting for ")
        || lower.starts_with("starting ")
        || lower.starts_with("retrying provider")
        || lower.starts_with("rate limited")
        || lower.starts_with("switch ")
        || lower.starts_with("opening a herdr pane ")
    {
        return true;
    }

    // Picker titles set as status when opening or navigating menus. Compared
    // case-insensitively because titles use display casing.
    matches!(
        lower.as_str(),
        "config · saves automatically"
            | "conversation tree"
            | "doctor diagnostics"
            | "inline shell"
            | "keyboard shortcuts"
            | "loaded agents"
            | "loaded skills"
            | "permission mode"
            | "refresh model lists"
            | "resume session"
            | "web search config"
            | "workspace rewind"
            | "workflows"
    )
}

/// Map free-form status text to a tone at the `set_status` boundary.
///
/// The overlay stores an explicit [`StatusTone`] and does not reclassify text.
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
            .style(Theme::surface().patch(tone_style(overlay.tone()))),
        popup,
    );
}

#[cfg(test)]
#[path = "status_overlay_tests.rs"]
mod tests;
