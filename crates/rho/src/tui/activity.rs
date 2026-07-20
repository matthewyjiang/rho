use std::time::{Duration, Instant};

use ratatui::text::{Line, Span};

use super::{render::display_width, theme::Theme};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActivityPhase {
    #[default]
    Starting,
    WaitingForProvider,
    Thinking,
    Responding,
    PreparingTool,
    RunningTool,
    RetryingProvider,
    Compacting,
    WaitingForApproval,
    WaitingForInput,
}

impl ActivityPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::WaitingForProvider => "waiting for provider",
            Self::Thinking => "thinking",
            Self::Responding => "responding",
            Self::PreparingTool => "preparing tool",
            Self::RunningTool => "running tool",
            Self::RetryingProvider => "retrying provider",
            Self::Compacting => "compacting context",
            Self::WaitingForApproval => "waiting for approval",
            Self::WaitingForInput => "waiting for input",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActivityStatus {
    Parent(ActivityPhase),
    Subagents(usize),
    ParentWithSubagents(ActivityPhase, usize),
}

impl ActivityStatus {
    pub(super) fn from_parent_and_subagents(
        parent: Option<ActivityPhase>,
        subagent_count: usize,
    ) -> Option<Self> {
        match (parent, subagent_count) {
            (Some(phase), 0) => Some(Self::Parent(phase)),
            (Some(phase), count) => Some(Self::ParentWithSubagents(phase, count)),
            (None, 0) => None,
            (None, count) => Some(Self::Subagents(count)),
        }
    }
}

pub(super) fn activity_width(available: usize, status: ActivityStatus) -> usize {
    let first_frame = LoadingSpinner::FRAMES[0];
    activity_labels(status)
        .into_iter()
        .find(|label| display_width(label) <= available)
        .map_or_else(
            || available.min(display_width(first_frame)),
            |label| display_width(&label),
        )
}

fn activity_labels(status: ActivityStatus) -> Vec<String> {
    let spinner = LoadingSpinner::FRAMES[0];
    let subagent_count = match status {
        ActivityStatus::Parent(_) => 0,
        ActivityStatus::Subagents(count) | ActivityStatus::ParentWithSubagents(_, count) => count,
    };
    let agents = if subagent_count == 1 {
        "1 agent".into()
    } else {
        format!("{subagent_count} agents")
    };
    match status {
        ActivityStatus::Parent(phase) => {
            vec![format!("{spinner} {}", phase.label()), spinner.into()]
        }
        ActivityStatus::ParentWithSubagents(phase, _) => vec![
            format!("{spinner} {}  ·  {agents}", phase.label()),
            format!("{spinner} {} · {subagent_count}", phase.label()),
            format!("{spinner} {subagent_count}"),
            spinner.into(),
        ],
        ActivityStatus::Subagents(_) => vec![
            format!("{spinner} {agents} working"),
            format!("{spinner} {subagent_count} agents"),
            format!("{spinner} {subagent_count}"),
            spinner.into(),
        ],
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct LoadingSpinner {
    started_at: Option<Instant>,
}

impl LoadingSpinner {
    const FRAMES: [&'static str; 6] = ["◜", "◠", "◝", "◞", "◡", "◟"];
    pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(95);

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
            .unwrap_or_else(|| Self::FRAMES[0].chars().take(available).collect());
        let Some(rest) = label.strip_prefix(Self::FRAMES[0]) else {
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
    // Leave enough room for the compact activity label when both controls share a row.
    let activity_width = usize::from(alongside_activity)
        * (display_width(LoadingSpinner::FRAMES[0]) + display_width(" 0") + 1);
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
