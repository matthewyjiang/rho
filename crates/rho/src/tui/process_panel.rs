use std::time::{Duration, Instant};

use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::{
    activity,
    linger_rail::{LingerRail, RailHit, RailItem, RailPointerPolicy},
    theme::Theme,
};
use crate::{
    subagent,
    tools::process::{LiveProcessSummary, ProcessManager, State},
};

/// Short hover hint shown on the right edge of a process row.
pub(super) const ACTION_HINT: &str = "peek";

/// A cargo compile step routinely goes ~60–90s without output; below this
/// "quiet" is noise — tripwire, tune after dogfooding.
pub(super) const QUIET_LABEL_AFTER: u64 = 60;
/// Past 5 minutes silent output usually means wedged — tripwire.
const QUIET_WARN_AFTER: u64 = 300;

/// A clickable process row resolved from a pointer position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProcessPeekTarget {
    pub(super) process_id: String,
}

/// Live managed processes shown in the activity rail.
#[derive(Clone)]
pub(super) struct ProcessPanel {
    rail: LingerRail<LiveProcessSummary>,
    manager: Option<ProcessManager>,
}

impl Default for ProcessPanel {
    fn default() -> Self {
        Self {
            rail: LingerRail::new(RailPointerPolicy::LiveOrLinger),
            manager: None,
        }
    }
}

impl ProcessPanel {
    pub(super) fn update(&mut self, manager: Option<&ProcessManager>, now: Instant) -> bool {
        self.manager = manager.cloned();
        let processes = manager
            .map(ProcessManager::live_summaries)
            .unwrap_or_default();
        self.ingest(processes, now)
    }

    pub(super) fn manager(&self) -> Option<&ProcessManager> {
        self.manager.as_ref()
    }

    pub(super) fn host_view(
        &self,
        process_id: &str,
    ) -> anyhow::Result<crate::tools::process::HostProcessView> {
        let manager = self
            .manager
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("process manager unavailable"))?;
        manager
            .host_view(process_id)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    pub(super) fn ingest(&mut self, processes: Vec<LiveProcessSummary>, now: Instant) -> bool {
        self.rail.ingest(processes, now)
    }

    pub(super) fn is_active(&self) -> bool {
        self.rail.is_active()
    }

    pub(super) fn live_count(&self) -> usize {
        self.rail.live_count()
    }

    pub(super) fn desired_height(&self) -> usize {
        self.rail.desired_height()
    }

    pub(super) fn clear_pointer_state(&mut self) {
        self.rail.clear_pointer_state();
    }

    pub(super) fn clear_pressed(&mut self) {
        self.rail.clear_pressed();
    }

    /// Returns whether the hovered process changed.
    pub(super) fn set_hovered(&mut self, process_id: Option<&str>) -> bool {
        self.rail.set_hovered(process_id)
    }

    /// Returns whether the pressed process changed.
    pub(super) fn set_pressed(&mut self, process_id: Option<&str>) -> bool {
        self.rail.set_pressed(process_id)
    }

    pub(super) fn pressed_process_id(&self) -> Option<&str> {
        self.rail.pressed_id()
    }

    pub(super) fn highlighted_row(&self, now: Instant) -> Option<(usize, activity::RailRowState)> {
        self.rail.highlighted_row(now)
    }

    pub(super) fn peek_target_at(
        &self,
        area: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) -> Option<ProcessPeekTarget> {
        match self.rail.hit_at(area, column, row, now)? {
            RailHit::Item(process_id) => Some(ProcessPeekTarget { process_id }),
            RailHit::Overflow => None,
        }
    }

    pub(super) fn lines(&self, width: usize, height: usize, now: Instant) -> Vec<Line<'static>> {
        if !self.rail.is_active() || width == 0 || height == 0 {
            return Vec::new();
        }

        let (rows, hidden) = self.rail.visible(height, now);
        let visible_count = rows.len() + usize::from(hidden.is_some());
        let mut lines = Vec::with_capacity(visible_count);
        for (offset, process) in rows.into_iter().enumerate() {
            let last = offset + 1 == visible_count;
            lines.push(process_line(
                process,
                activity::tree_connector(last),
                width,
                self.rail
                    .row_state(&process.process_id, process.state.is_live()),
            ));
        }
        if let Some(hidden) = hidden {
            lines.push(summary_line(
                activity::overflow_label(hidden, "job", "jobs"),
                activity::tree_connector(true),
                width,
            ));
        }
        lines
    }
}

impl RailItem for LiveProcessSummary {
    fn id(&self) -> &str {
        &self.process_id
    }

    fn is_live(&self) -> bool {
        self.state.is_live()
    }

    fn is_failure(&self) -> bool {
        !self.state.is_live() && !is_process_success(self)
    }

    fn linger(&self) -> Duration {
        if is_process_success(self) {
            activity::LINGER_OK
        } else {
            activity::LINGER_FAIL
        }
    }
}

fn process_line(
    process: &LiveProcessSummary,
    connector: &'static str,
    width: usize,
    row_state: activity::RailRowState,
) -> Line<'static> {
    let command = command_identity(&process.command);
    let elapsed = subagent::format_elapsed_secs(process.elapsed_seconds);
    let trailing = match row_state {
        activity::RailRowState::Idle => elapsed,
        activity::RailRowState::Hovered | activity::RailRowState::Pressed => {
            format!("⏎ {ACTION_HINT} · {elapsed}")
        }
    };
    let trailing_style = match row_state {
        activity::RailRowState::Idle => process_trailing_style(process),
        activity::RailRowState::Hovered | activity::RailRowState::Pressed => Theme::dim(),
    };
    let row_style = Theme::activity_rail_row(row_state);
    let (activity_text, activity_style) = process_activity(process);
    activity::RailRow {
        connector,
        identity: vec![
            Span::styled(
                activity::PROCESS_GLYPH,
                Theme::text_strong().patch(row_style),
            ),
            Span::styled(command.to_owned(), Theme::text_strong().patch(row_style)),
        ],
        activity: activity_text,
        activity_style,
        trailing,
        trailing_style,
        row_style,
    }
    .into_line(width)
}

fn summary_line(label: String, connector: &'static str, width: usize) -> Line<'static> {
    let row_style = Theme::activity_rail();
    activity::RailRow {
        connector,
        identity: vec![Span::styled(label, Theme::dim().patch(row_style))],
        activity: String::new(),
        activity_style: Theme::dim(),
        trailing: String::new(),
        trailing_style: Theme::dim(),
        row_style,
    }
    .into_line(width)
}

pub(super) fn command_identity(command: &str) -> &str {
    command.lines().next().unwrap_or(command)
}

fn is_process_success(process: &LiveProcessSummary) -> bool {
    matches!(process.state, State::Exited) && process.exit_code == Some(0)
}

pub(super) fn process_activity(process: &LiveProcessSummary) -> (String, ratatui::style::Style) {
    process_activity_for(process.state, process.quiet_seconds, process.exit_code)
}

pub(super) fn process_activity_for(
    state: State,
    quiet_seconds: Option<u64>,
    exit_code: Option<i32>,
) -> (String, ratatui::style::Style) {
    match state {
        State::Starting => ("starting".into(), Theme::text()),
        State::Running => match quiet_seconds {
            Some(quiet) if quiet >= QUIET_LABEL_AFTER => (
                format!("quiet {}", subagent::format_elapsed_secs(quiet)),
                Theme::text(),
            ),
            _ => ("running".into(), Theme::text()),
        },
        State::Exited => match exit_code {
            Some(0) => ("✓ exit 0".into(), Theme::activity_rail_success()),
            Some(code) => (format!("✗ exit {code}"), Theme::activity_rail_error()),
            None => ("✗ exited".into(), Theme::activity_rail_error()),
        },
        State::Terminated => ("✗ terminated".into(), Theme::activity_rail_error()),
        State::TimedOut => ("✗ timed out".into(), Theme::activity_rail_error()),
        State::FailedToStart => ("✗ failed to start".into(), Theme::activity_rail_error()),
    }
}

fn process_trailing_style(process: &LiveProcessSummary) -> ratatui::style::Style {
    match process.quiet_seconds {
        Some(quiet) if process.state.is_live() && quiet >= QUIET_WARN_AFTER => {
            Theme::activity_rail_warning()
        }
        _ => Theme::dim(),
    }
}

#[cfg(test)]
#[path = "process_panel_tests.rs"]
mod tests;
