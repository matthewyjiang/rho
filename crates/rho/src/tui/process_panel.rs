use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{activity, theme::Theme, App};
use crate::{
    subagent,
    tools::process::{LiveProcessSummary, ProcessManager, State},
};

/// A cargo compile step routinely goes ~60–90s without output; below this
/// "quiet" is noise — tripwire, tune after dogfooding.
const QUIET_LABEL_AFTER: u64 = 60;
/// Past 5 minutes silent output usually means wedged — tripwire.
const QUIET_WARN_AFTER: u64 = 300;

/// Live managed processes shown in the activity rail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProcessPanel {
    processes: Vec<LiveProcessSummary>,
    terminal_seen: HashMap<String, Instant>,
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

    pub(super) fn lines(&self, width: usize, height: usize, now: Instant) -> Vec<Line<'static>> {
        if self.processes.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let processes = self.visible_processes(now);
        let (indices, hidden) = activity::select_capped_rail_rows(
            &processes,
            height,
            |process| is_live(process.state),
            |process| is_lingering_failure(process),
        );
        let summary = hidden.map(|count| activity::overflow_label(count, "job", "jobs"));
        let visible_count = indices.len() + usize::from(summary.is_some());
        let mut lines = Vec::with_capacity(visible_count);
        for (offset, index) in indices.into_iter().enumerate() {
            let is_last = offset + 1 == visible_count;
            let connector = activity::tree_connector(is_last);
            lines.push(process_line(processes[index], connector, width));
        }
        if let Some(label) = summary {
            lines.push(summary_line(label, activity::tree_connector(true), width));
        }
        lines
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
    }
}

fn process_line(
    process: &LiveProcessSummary,
    connector: &'static str,
    width: usize,
) -> Line<'static> {
    let command = command_identity(&process.command);
    let row_style = Theme::activity_rail();
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
        trailing: subagent::format_elapsed_secs(process.elapsed_seconds),
        trailing_style: process_trailing_style(process),
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

fn command_identity(command: &str) -> &str {
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

fn process_activity(process: &LiveProcessSummary) -> (String, ratatui::style::Style) {
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
