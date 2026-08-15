use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
    App,
};
use crate::{
    subagent,
    tools::process::{LiveProcessSummary, ProcessManager, State},
};

// Same visible-row and content-width receipts as `subagent_panel`.
const MAX_VISIBLE_PROCESSES: usize = 2;
const MAX_PROCESS_CONTENT_WIDTH: usize = 52;

/// Live managed processes shown in the activity rail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProcessPanel {
    processes: Vec<LiveProcessSummary>,
}

impl ProcessPanel {
    pub(super) fn update(&mut self, manager: Option<&ProcessManager>) -> bool {
        let processes = manager
            .map(ProcessManager::live_summaries)
            .unwrap_or_default();
        self.replace_processes(processes)
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

    pub(super) fn desired_height(&self) -> usize {
        self.processes.len().min(MAX_VISIBLE_PROCESSES)
    }

    pub(super) fn lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        if self.processes.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let processes = self.visible_processes(height);
        let visible_count = processes.len();
        let mut lines = Vec::with_capacity(visible_count);
        for (index, process) in processes.into_iter().enumerate() {
            let Some(activity) = state_label(process.state) else {
                continue;
            };
            let connector = super::activity::tree_connector(index + 1 == visible_count);
            lines.push(process_line(process, activity, connector, width));
        }
        lines
    }

    fn visible_processes(&self, height: usize) -> Vec<&LiveProcessSummary> {
        let limit = self.processes.len().min(MAX_VISIBLE_PROCESSES).min(height);
        self.processes
            .iter()
            .filter(|process| matches!(process.state, State::Starting | State::Running))
            .take(limit)
            .collect()
    }
}

impl App {
    pub(super) fn render_process_rail(&self, frame: &mut Frame<'_>, area: Rect, width: usize) {
        if area.height == 0 {
            return;
        }
        frame.render_widget(
            Paragraph::new(self.process_panel.lines(width, area.height as usize))
                .style(Theme::activity_rail()),
            area,
        );
    }
}

fn process_line(
    process: &LiveProcessSummary,
    activity: &str,
    connector: &'static str,
    width: usize,
) -> Line<'static> {
    const SEPARATOR: &str = "  ·  ";
    const MIN_GAP: usize = 2;

    let connector_width = display_width(connector);
    let content_width = width
        .saturating_sub(connector_width)
        .min(MAX_PROCESS_CONTENT_WIDTH);
    let command = command_identity(&process.command);
    let short_id = short_process_id(&process.process_id);
    let identity_width = display_width(command) + 1 + display_width(short_id);
    let separator_width = display_width(SEPARATOR);
    let elapsed = subagent::format_elapsed_secs(process.elapsed_seconds);
    let fixed_width = identity_width + separator_width + MIN_GAP + display_width(&elapsed);
    let row_style = Theme::activity_rail();

    if fixed_width >= content_width {
        let detail = truncate_one_line(
            &format!("{command} {short_id}{SEPARATOR}{activity}  {elapsed}"),
            content_width,
        );
        return Line::from(vec![
            Span::styled(connector, Theme::dim().patch(row_style)),
            Span::styled(detail, Theme::dim().patch(row_style)),
        ]);
    }

    let activity_width = content_width.saturating_sub(fixed_width);
    let activity = truncate_one_line(activity, activity_width);
    let gap = " ".repeat(content_width.saturating_sub(
        identity_width + separator_width + display_width(&activity) + display_width(&elapsed),
    ));
    Line::from(vec![
        Span::styled(connector, Theme::dim().patch(row_style)),
        Span::styled(command.to_owned(), Theme::text_strong().patch(row_style)),
        Span::styled(" ", row_style),
        Span::styled(short_id.to_owned(), Theme::dim().patch(row_style)),
        Span::styled(SEPARATOR, Theme::dim().patch(row_style)),
        Span::styled(activity, Theme::text().patch(row_style)),
        Span::styled(gap, row_style),
        Span::styled(elapsed, Theme::dim().patch(row_style)),
    ])
}

fn command_identity(command: &str) -> &str {
    command.lines().next().unwrap_or(command)
}

fn short_process_id(process_id: &str) -> &str {
    process_id
        .get(..8.min(process_id.len()))
        .unwrap_or(process_id)
}

fn state_label(state: State) -> Option<&'static str> {
    match state {
        State::Starting => Some("starting"),
        State::Running => Some("running"),
        State::Exited | State::Terminated | State::TimedOut | State::FailedToStart => None,
    }
}

#[cfg(test)]
#[path = "process_panel_tests.rs"]
mod tests;
