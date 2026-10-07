//! Session-owned workflows in the compact activity tree.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::{
    activity,
    render::{display_width, truncate_one_line},
    stacked_rail::{RailHit, RailItem, RailPointerPolicy, StackedRail},
    theme::Theme,
};
use crate::{
    subagent::format_elapsed_secs,
    tools::workflow_tracker::{WorkflowRailSummary, WorkflowRunTracker},
    workflow::{RunLifecycle, WorkflowOutcome},
};

#[derive(Clone)]
pub(super) struct WorkflowPanel {
    rail: StackedRail<WorkflowRailSummary>,
    pending_watch: Option<String>,
    source: Option<(WorkflowRunTracker, String)>,
}

impl Default for WorkflowPanel {
    fn default() -> Self {
        Self {
            rail: StackedRail::new(RailPointerPolicy::LiveOrFinished),
            pending_watch: None,
            source: None,
        }
    }
}

impl WorkflowPanel {
    pub(super) fn update(&mut self, tracker: &WorkflowRunTracker, session_id: &str) -> bool {
        self.source = Some((tracker.clone(), session_id.to_owned()));
        self.rail.ingest(tracker.rail_summaries(session_id))
    }

    /// Delivery may drain notifications between a panel tick and a draw. Read
    /// the in-memory source again so rows leave in the delivery repaint.
    pub(super) fn refresh(&mut self) {
        if let Some((tracker, session_id)) = &self.source {
            self.rail.ingest(tracker.rail_summaries(session_id));
        }
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

    pub(super) fn set_hovered(&mut self, run_id: Option<&str>) {
        self.rail.set_hovered(run_id);
    }

    pub(super) fn set_pressed(&mut self, run_id: Option<&str>) {
        self.rail.set_pressed(run_id);
    }

    pub(super) fn pressed_run_id(&self) -> Option<&str> {
        self.rail.pressed_id()
    }

    pub(super) fn highlighted_row(&self, height: usize) -> Option<(usize, activity::RailRowState)> {
        self.rail.highlighted_row(height)
    }

    pub(super) fn watch_target_at(&self, area: Rect, column: u16, row: u16) -> Option<String> {
        match self.rail.hit_at(area, column, row)? {
            RailHit::Item(run_id) => Some(run_id),
            RailHit::Overflow => None,
        }
    }

    /// Pointer routing is synchronous. Only idle input clicks may arm this;
    /// the idle loop rechecks eligibility and drops stale requests.
    pub(super) fn request_watch(&mut self, run_id: String) {
        self.pending_watch = Some(run_id);
    }

    pub(super) fn take_watch_request(&mut self) -> Option<String> {
        self.pending_watch.take()
    }

    pub(super) fn lines(
        &self,
        width: usize,
        height: usize,
        continues_below: bool,
    ) -> Vec<Line<'static>> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let (rows, hidden) = self.rail.visible(height);
        let visible_count = rows.len() + usize::from(hidden.is_some());
        let mut lines = Vec::with_capacity(visible_count);
        for (index, run) in rows.into_iter().enumerate() {
            let row_state = self.rail.row_state(&run.run_id, run.is_live());
            let row_style = Theme::activity_rail_row(row_state);
            let elapsed = format_elapsed_secs(run.elapsed_seconds);
            let mut trailing = match row_state {
                activity::RailRowState::Idle => elapsed,
                activity::RailRowState::Hovered | activity::RailRowState::Pressed => {
                    format!("⏎ watch · {elapsed}")
                }
            };
            let (mut status, status_style) = workflow_status(run);
            let lifecycle_width = display_width(&status);
            if run.total_tasks > 0 {
                status.push_str(&format!(
                    " · {}/{} tasks",
                    run.completed_tasks, run.total_tasks
                ));
            }
            let progress_width = display_width(&status);
            if let Some(task) = run.active_task.as_ref().filter(|_| run.is_live()) {
                status.push_str(&format!(" · {task}"));
            }
            let connector =
                activity::tree_connector(index + 1 == visible_count && !continues_below);
            let content_width = width.saturating_sub(display_width(connector));
            // RailRow normally prioritizes identity and elapsed. Workflows need
            // lifecycle first: reserve the measured lifecycle/progress column,
            // then truncate the name and drop elapsed when it would hide status.
            // These separators mirror RailRow's actual column spacing.
            let separator_width = display_width("  ·  ");
            let trailing_gap = display_width("  ");
            let minimum_identity = display_width("…");
            let minimum_status_row = lifecycle_width + separator_width + minimum_identity;
            if content_width < minimum_status_row {
                lines.push(
                    activity::RailRow {
                        connector,
                        identity: vec![Span::styled(status, row_style.patch(status_style))],
                        activity: String::new(),
                        activity_style: status_style,
                        trailing: String::new(),
                        trailing_style: Theme::activity_rail_dim(),
                        row_style,
                    }
                    .into_line(width),
                );
                continue;
            }
            let mut trailing_reserve = trailing_gap + display_width(&trailing);
            if content_width < minimum_status_row + trailing_reserve {
                trailing.clear();
                trailing_reserve = 0;
            }
            let status_reserve = progress_width.min(
                content_width.saturating_sub(trailing_reserve + separator_width + minimum_identity),
            );
            let identity_budget =
                content_width.saturating_sub(trailing_reserve + separator_width + status_reserve);
            lines.push(
                activity::RailRow {
                    connector,
                    identity: vec![Span::styled(
                        truncate_one_line(
                            &format!("workflow {}", run.workflow_name),
                            identity_budget,
                        ),
                        Theme::text_strong().patch(row_style),
                    )],
                    activity: status,
                    activity_style: status_style,
                    trailing,
                    trailing_style: Theme::activity_rail_dim(),
                    row_style,
                }
                .into_line(width),
            );
        }
        if let Some(hidden) = hidden {
            lines.push(
                activity::RailRow {
                    connector: activity::tree_connector(!continues_below),
                    identity: vec![Span::styled(
                        activity::overflow_label(hidden, "workflow", "workflows"),
                        Theme::activity_rail_dim(),
                    )],
                    activity: String::new(),
                    activity_style: Theme::activity_rail_dim(),
                    trailing: String::new(),
                    trailing_style: Theme::activity_rail_dim(),
                    row_style: Theme::activity_rail(),
                }
                .into_line(width),
            );
        }
        lines
    }
}

impl super::App {
    /// Watching suspends the terminal and replaces the composer. Never defer a
    /// busy-turn click or drop a form/picker to honor an earlier pointer event.
    pub(super) fn can_open_workflow_watch_from_rail(&self) -> bool {
        !self.is_ui_busy() && matches!(self.input_ui.composer(), super::ComposerMode::Input)
    }
}

impl RailItem for WorkflowRailSummary {
    fn id(&self) -> &str {
        &self.run_id
    }

    fn is_live(&self) -> bool {
        self.is_live()
    }

    fn is_failure(&self) -> bool {
        self.is_failure()
    }
}

fn workflow_status(run: &WorkflowRailSummary) -> (String, ratatui::style::Style) {
    if run.failed {
        return ("✗ failed".into(), Theme::activity_rail_error());
    }
    match run.lifecycle {
        RunLifecycle::Planned => ("starting".into(), Theme::text()),
        RunLifecycle::Running => ("running".into(), Theme::text()),
        RunLifecycle::Cancelling => ("cancelling".into(), Theme::activity_rail_warning()),
        RunLifecycle::NeedsRecovery => ("needs recovery".into(), Theme::activity_rail_warning()),
        RunLifecycle::Completed => match run.outcome {
            Some(WorkflowOutcome::Success) => ("✓ done".into(), Theme::activity_rail_success()),
            Some(WorkflowOutcome::Failure) => ("✗ failed".into(), Theme::activity_rail_error()),
            Some(WorkflowOutcome::Denial) => ("✗ denied".into(), Theme::activity_rail_error()),
            Some(WorkflowOutcome::Cancellation) => ("cancelled".into(), Theme::activity_rail_dim()),
            Some(WorkflowOutcome::Blocked) => ("✗ blocked".into(), Theme::activity_rail_error()),
            None => ("finished".into(), Theme::text()),
        },
    }
}
