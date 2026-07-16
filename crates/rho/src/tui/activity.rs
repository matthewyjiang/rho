use std::time::{Duration, Instant};

use ratatui::text::{Line, Span};

use super::{render::display_width, theme::Theme};

const WORKING_LABEL: &str = "⠋ working";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActivityStatus {
    Working,
    Subagents(usize),
    WorkingWithSubagents(usize),
}

pub(super) fn activity_width(available: usize, status: ActivityStatus) -> usize {
    activity_labels(status)
        .into_iter()
        .find(|label| display_width(label) <= available)
        .map_or_else(
            || available.min(display_width("⠋")),
            |label| display_width(&label),
        )
}

fn activity_labels(status: ActivityStatus) -> Vec<String> {
    let subagent_count = match status {
        ActivityStatus::Working => 0,
        ActivityStatus::Subagents(count) | ActivityStatus::WorkingWithSubagents(count) => count,
    };
    let agents = if subagent_count == 1 {
        "1 subagent".into()
    } else {
        format!("{subagent_count} subagents")
    };
    match status {
        ActivityStatus::Working => vec![WORKING_LABEL.into(), "⠋".into()],
        ActivityStatus::WorkingWithSubagents(_) => vec![
            format!("{WORKING_LABEL}  ·  {agents}"),
            format!("⠋ working · {subagent_count}"),
            format!("⠋ {subagent_count}"),
            "⠋".into(),
        ],
        ActivityStatus::Subagents(_) => vec![
            format!("⠋ {agents} working"),
            format!("⠋ {subagent_count} agents"),
            format!("⠋ {subagent_count}"),
            "⠋".into(),
        ],
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

    pub(super) fn line(
        &self,
        now: Instant,
        available: usize,
        status: ActivityStatus,
    ) -> Line<'static> {
        let label = activity_labels(status)
            .into_iter()
            .find(|label| display_width(label) <= available)
            .unwrap_or_else(|| "⠋".chars().take(available).collect());
        let Some(rest) = label.strip_prefix('⠋') else {
            return Line::default();
        };
        Line::from(vec![
            Span::styled(self.frame_at(now), Theme::accent()),
            Span::styled(rest.to_string(), Theme::dim()),
        ])
    }
}

pub(super) fn jump_to_bottom_text(width: usize, binding: &str, alongside_activity: bool) -> String {
    let full = format!("↓ jump to bottom  {binding}");
    let compact = format!("↓ bottom {binding}");
    let shortcut = format!("↓ {binding}");
    let activity_width = usize::from(alongside_activity) * (display_width(WORKING_LABEL) + 1);
    let available = width.saturating_sub(activity_width);

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
