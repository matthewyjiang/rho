use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{activity, process_peek, theme::Theme, App};
use crate::{
    subagent,
    tools::process::{LiveProcessSummary, ProcessManager, State},
};

/// A cargo compile step routinely goes ~60–90s without output; below this
/// "quiet" is noise — tripwire, tune after dogfooding.
const QUIET_LABEL_AFTER: u64 = 60;
/// Past 5 minutes silent output usually means wedged — tripwire.
const QUIET_WARN_AFTER: u64 = 300;

/// A clickable process row resolved from a pointer position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProcessPeekTarget {
    pub(super) process_id: String,
}

/// Live managed processes shown in the activity rail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProcessPanel {
    processes: Vec<LiveProcessSummary>,
    terminal_seen: HashMap<String, Instant>,
    hovered_process_id: Option<String>,
    pressed_process_id: Option<String>,
}

impl ProcessPanel {
    pub(super) fn update(&mut self, manager: Option<&ProcessManager>, now: Instant) -> bool {
        let processes = manager
            .map(ProcessManager::live_summaries)
            .unwrap_or_default();
        self.ingest(processes, now)
    }

    fn ingest(&mut self, processes: Vec<LiveProcessSummary>, now: Instant) -> bool {
        let incoming: HashSet<String> = processes
            .iter()
            .map(|process| process.process_id.clone())
            .collect();
        self.terminal_seen.retain(|id, _| incoming.contains(id));
        let processes = processes
            .into_iter()
            .filter(|process| self.keep_process(process, now))
            .collect();
        self.replace_processes(processes)
    }

    fn keep_process(&mut self, process: &LiveProcessSummary, now: Instant) -> bool {
        if is_live(process.state) {
            self.terminal_seen.remove(&process.process_id);
            return true;
        }
        let first_seen = *self
            .terminal_seen
            .entry(process.process_id.clone())
            .or_insert(now);
        activity::linger_active(first_seen, now, linger_for_process(process))
    }

    fn replace_processes(&mut self, processes: Vec<LiveProcessSummary>) -> bool {
        if self.processes == processes {
            return false;
        }
        self.processes = processes;
        let process_is_shown = |process_id: &str| {
            self.processes
                .iter()
                .any(|process| process.process_id == process_id)
        };
        if !self
            .hovered_process_id
            .as_deref()
            .is_some_and(process_is_shown)
        {
            self.hovered_process_id = None;
        }
        if !self
            .pressed_process_id
            .as_deref()
            .is_some_and(process_is_shown)
        {
            self.pressed_process_id = None;
        }
        true
    }

    pub(super) fn is_active(&self) -> bool {
        !self.processes.is_empty()
    }

    pub(super) fn live_count(&self) -> usize {
        self.processes
            .iter()
            .filter(|process| is_live(process.state))
            .count()
    }

    pub(super) fn desired_height(&self) -> usize {
        self.processes.len().min(activity::MAX_VISIBLE_RAIL_ROWS)
    }

    pub(super) fn clear_pointer_state(&mut self) {
        self.hovered_process_id = None;
        self.pressed_process_id = None;
    }

    /// Returns whether the hovered process changed.
    pub(super) fn set_hovered(&mut self, process_id: Option<&str>) -> bool {
        if self.hovered_process_id.as_deref() == process_id {
            return false;
        }
        self.hovered_process_id = process_id.map(str::to_owned);
        true
    }

    /// Returns whether the pressed process changed.
    pub(super) fn set_pressed(&mut self, process_id: Option<&str>) -> bool {
        if self.pressed_process_id.as_deref() == process_id {
            return false;
        }
        self.pressed_process_id = process_id.map(str::to_owned);
        true
    }

    pub(super) fn pressed_process_id(&self) -> Option<&str> {
        self.pressed_process_id.as_deref()
    }

    pub(super) fn highlighted_row(&self) -> Option<(usize, activity::RailRowState)> {
        let rows = self.visible_rows(activity::MAX_VISIBLE_RAIL_ROWS);
        let row_for = |process_id: &str| {
            rows.iter().position(|row| match row {
                VisibleProcessRow::Process(process) => process.process_id == process_id,
                VisibleProcessRow::Overflow { .. } => false,
            })
        };
        if let Some(row) = self.pressed_process_id.as_deref().and_then(row_for) {
            return Some((row, activity::RailRowState::Pressed));
        }
        self.hovered_process_id
            .as_deref()
            .and_then(row_for)
            .map(|row| (row, activity::RailRowState::Hovered))
    }

    pub(super) fn peek_target_at(
        &self,
        area: Rect,
        column: u16,
        row: u16,
    ) -> Option<ProcessPeekTarget> {
        if !area.contains(Position { x: column, y: row }) || area.height == 0 {
            return None;
        }
        let index = row.saturating_sub(area.y) as usize;
        match self.visible_rows(area.height as usize).get(index)? {
            VisibleProcessRow::Process(process) => Some(ProcessPeekTarget {
                process_id: process.process_id.clone(),
            }),
            VisibleProcessRow::Overflow { .. } => None,
        }
    }

    pub(super) fn lines(&self, width: usize, height: usize, now: Instant) -> Vec<Line<'static>> {
        if self.processes.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let rows = self.visible_rows_at(height, now);
        let visible_count = rows.len();
        let mut lines = Vec::with_capacity(visible_count);
        for (offset, row) in rows.into_iter().enumerate() {
            let connector = activity::tree_connector(offset + 1 == visible_count);
            lines.push(match row {
                VisibleProcessRow::Process(process) => {
                    process_line(process, connector, width, self.row_state(process))
                }
                VisibleProcessRow::Overflow { hidden } => summary_line(
                    activity::overflow_label(hidden, "job", "jobs"),
                    connector,
                    width,
                ),
            });
        }
        lines
    }

    fn row_state(&self, process: &LiveProcessSummary) -> activity::RailRowState {
        if self.pressed_process_id.as_deref() == Some(process.process_id.as_str()) {
            activity::RailRowState::Pressed
        } else if self.hovered_process_id.as_deref() == Some(process.process_id.as_str()) {
            activity::RailRowState::Hovered
        } else {
            activity::RailRowState::Idle
        }
    }

    fn visible_rows(&self, height: usize) -> Vec<VisibleProcessRow<'_>> {
        self.visible_rows_at(height, Instant::now())
    }

    fn visible_rows_at(&self, height: usize, now: Instant) -> Vec<VisibleProcessRow<'_>> {
        let processes = self.visible_processes(now);
        let (indices, hidden) = activity::select_capped_rail_rows(
            &processes,
            height,
            |process| is_live(process.state),
            |process| is_lingering_failure(process),
        );
        let mut rows: Vec<VisibleProcessRow<'_>> = indices
            .into_iter()
            .map(|index| VisibleProcessRow::Process(processes[index]))
            .collect();
        if let Some(hidden) = hidden {
            rows.push(VisibleProcessRow::Overflow { hidden });
        }
        rows
    }

    fn visible_processes(&self, now: Instant) -> Vec<&LiveProcessSummary> {
        self.processes
            .iter()
            .filter(|process| self.row_visible(process, now))
            .collect()
    }

    fn row_visible(&self, process: &LiveProcessSummary, now: Instant) -> bool {
        if is_live(process.state) {
            return true;
        }
        self.terminal_seen
            .get(&process.process_id)
            .is_some_and(|first| activity::linger_active(*first, now, linger_for_process(process)))
    }
}

impl App {
    pub(super) fn render_process_rail(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        width: usize,
        now: Instant,
    ) {
        if area.height == 0 {
            return;
        }
        frame.render_widget(
            Paragraph::new(self.process_panel.lines(width, area.height as usize, now))
                .style(Theme::activity_rail()),
            area,
        );
        if let Some((row, state)) = self.process_panel.highlighted_row() {
            let y = area.y.saturating_add(row as u16);
            if y < area.bottom() {
                for x in area.x..area.right() {
                    frame.buffer_mut()[(x, y)].set_style(Theme::activity_rail_row(state));
                }
            }
        }
    }
}

enum VisibleProcessRow<'a> {
    Process(&'a LiveProcessSummary),
    Overflow { hidden: usize },
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
            format!("⏎ {} · {elapsed}", process_peek::ACTION_HINT)
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

fn is_live(state: State) -> bool {
    matches!(state, State::Starting | State::Running)
}

fn is_lingering_failure(process: &LiveProcessSummary) -> bool {
    !is_live(process.state) && !is_process_success(process)
}

fn is_process_success(process: &LiveProcessSummary) -> bool {
    matches!(process.state, State::Exited) && process.exit_code == Some(0)
}

fn linger_for_process(process: &LiveProcessSummary) -> Duration {
    if is_process_success(process) {
        activity::LINGER_OK
    } else {
        activity::LINGER_FAIL
    }
}

pub(super) fn process_activity(process: &LiveProcessSummary) -> (String, ratatui::style::Style) {
    match process.state {
        State::Starting => ("starting".into(), Theme::text()),
        State::Running => match process.quiet_seconds {
            Some(quiet) if quiet >= QUIET_LABEL_AFTER => (
                format!("quiet {}", subagent::format_elapsed_secs(quiet)),
                Theme::text(),
            ),
            _ => ("running".into(), Theme::text()),
        },
        State::Exited => match process.exit_code {
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
        Some(quiet) if is_live(process.state) && quiet >= QUIET_WARN_AFTER => {
            Theme::activity_rail_warning()
        }
        _ => Theme::dim(),
    }
}

#[cfg(test)]
#[path = "process_panel_tests.rs"]
mod tests;
