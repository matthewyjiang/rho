use std::time::{Duration, Instant};

use ratatui::text::{Line, Span};

use super::{render::display_width, theme::Theme};

const SPINNER_LABEL: &str = "⠋ working";

pub(super) fn spinner_width(available: usize) -> usize {
    if available >= display_width(SPINNER_LABEL) {
        display_width(SPINNER_LABEL)
    } else {
        available.min(display_width("⠋"))
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct LoadingSpinner {
    started_at: Option<Instant>,
}

impl LoadingSpinner {
    const FRAMES: [&'static str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(80);

    pub(super) fn start(&mut self) {
        self.started_at = Some(Instant::now());
    }

    pub(super) fn start_if_needed(&mut self) {
        if self.started_at.is_none() {
            self.start();
        }
    }

    pub(super) fn stop(&mut self) {
        self.started_at = None;
    }

    fn frame_at(&self, now: Instant) -> &'static str {
        let Some(started_at) = self.started_at else {
            return Self::FRAMES[0];
        };
        let interval_ms = Self::FRAME_INTERVAL.as_millis().max(1);
        let frame = now
            .saturating_duration_since(started_at)
            .as_millis()
            .checked_div(interval_ms)
            .unwrap_or(0) as usize;
        Self::FRAMES[frame % Self::FRAMES.len()]
    }

    pub(super) fn line(&self, now: Instant, available: usize) -> Line<'static> {
        if available >= display_width(SPINNER_LABEL) {
            Line::from(vec![
                Span::styled(self.frame_at(now), Theme::accent()),
                Span::styled(" working", Theme::dim()),
            ])
        } else if available > 0 {
            Line::from(Span::styled(self.frame_at(now), Theme::accent()))
        } else {
            Line::default()
        }
    }
}

pub(super) fn jump_to_bottom_text(width: usize, binding: &str, alongside_spinner: bool) -> String {
    let full = format!("↓ jump to bottom  {binding}");
    let compact = format!("↓ bottom {binding}");
    let shortcut = format!("↓ {binding}");
    let spinner_width = usize::from(alongside_spinner) * (display_width(SPINNER_LABEL) + 1);
    let available = width.saturating_sub(spinner_width);

    if display_width(&full) <= available {
        full
    } else if display_width(&compact) <= available {
        compact
    } else if display_width(&shortcut) <= width {
        shortcut
    } else {
        super::render::truncate_one_line(&shortcut, width)
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
