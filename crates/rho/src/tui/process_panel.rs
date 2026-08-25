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

    pub(super) fn live_count(&self) -> usize {
        self.processes
            .iter()
            .filter(|process| matches!(process.state, State::Starting | State::Running))
            .count()
    }

    pub(super) fn desired_height(&self) -> usize {
        self.processes.len().min(activity::MAX_VISIBLE_RAIL_ROWS)
    }

    pub(super) fn lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        if self.processes.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let processes = self.visible_processes(height);
        let visible_count = processes.len();
        processes
            .iter()
            .enumerate()
            .map(|(index, process)| {
                let connector = activity::tree_connector(index + 1 == visible_count);
                process_line(process, state_label(process.state), connector, width)
            })
            .collect()
    }

    fn visible_processes(&self, height: usize) -> &[LiveProcessSummary] {
        let limit = self
            .processes
            .len()
            .min(activity::MAX_VISIBLE_RAIL_ROWS)
            .min(height);
        &self.processes[..limit]
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
    activity_text: &str,
    connector: &'static str,
    width: usize,
) -> Line<'static> {
    let command = command_identity(&process.command);
    let short_id = short_process_id(&process.process_id);
    let row_style = Theme::activity_rail();
    activity::RailRow {
        connector,
        identity: vec![
            Span::styled(command.to_owned(), Theme::text_strong().patch(row_style)),
            Span::styled(" ", row_style),
            Span::styled(short_id.to_owned(), Theme::dim().patch(row_style)),
        ],
        activity: activity_text.to_owned(),
        trailing: subagent::format_elapsed_secs(process.elapsed_seconds),
        row_style,
    }
    .into_line(width)
}

fn command_identity(command: &str) -> &str {
    command.lines().next().unwrap_or(command)
}

fn short_process_id(process_id: &str) -> &str {
    process_id
        .get(..8.min(process_id.len()))
        .unwrap_or(process_id)
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Starting => "starting",
        State::Running => "running",
        // `live_summaries` never yields terminal states; still label so the
        // reserved rail row cannot go blank.
        State::Exited | State::Terminated | State::TimedOut | State::FailedToStart => "done",
    }
}

#[cfg(test)]
#[path = "process_panel_tests.rs"]
mod tests;
