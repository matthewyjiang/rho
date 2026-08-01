use std::time::Instant;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::workflow::{
    CommandExit, NodeState, NodeTerminalState, RunLifecycle, WorkflowOutcome, WorkspaceAccess,
};

use super::{
    control::ConfirmKind,
    dag::{self, state_glyph, state_label, state_style},
    event_adapter::{CancellationState, ExecutionMetadata, TerminalReason, WorkflowNodeSnapshot},
    state::WorkflowUiState,
};

// Receipt: three rows show one output row plus enough context to make scrolling clear.
const MIN_DETAILS_BODY_ROWS: u16 = 3;

pub(super) fn draw(frame: &mut Frame<'_>, state: &mut WorkflowUiState) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, vertical[0], state);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(vertical[1]);
    draw_dag(frame, body[0], state);
    draw_details(frame, body[1], state);
    draw_footer(frame, vertical[2], state);
}

pub(super) fn handle_mouse(
    state: &mut WorkflowUiState,
    kind: crossterm::event::MouseEventKind,
    column: u16,
    row: u16,
) -> bool {
    state.details_mut().handle_mouse(kind, column, row)
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &WorkflowUiState) {
    let snapshot = state.snapshot();
    let progress = progress_summary(state);
    let status = run_status_label(snapshot.lifecycle, snapshot.outcome, snapshot.cancellation);
    let line = Line::from(vec![
        Span::styled(
            snapshot.workflow_name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  ·  {status}  ·  {progress}")),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_dag(frame: &mut Frame<'_>, area: Rect, state: &WorkflowUiState) {
    let inner_width = area.width.saturating_sub(2);
    let activities = state
        .snapshot()
        .nodes
        .iter()
        .map(|node| node_graph_activity(node, state))
        .collect::<Vec<_>>();
    let dag_lines = dag::render_dag(
        &state.snapshot().nodes,
        state.selected_index(),
        inner_width,
        &activities,
    );
    let lines = dag::to_paragraph_lines(dag_lines);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(" Graph ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_details(frame: &mut Frame<'_>, area: Rect, state: &mut WorkflowUiState) {
    let block = Block::default().title(" Selected ").borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(node) = state.selected_node().cloned() else {
        frame.render_widget(Paragraph::new("No steps"), inner);
        state.details_mut().sync_geometry(Rect::default(), 0, 0);
        return;
    };

    let meta = detail_meta_lines(&node, state);
    let (meta_height, _) = detail_pane_heights(&meta, inner, state.details().has_body());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(meta_height), Constraint::Min(0)])
        .split(inner);

    frame.render_widget(Paragraph::new(meta).wrap(Wrap { trim: false }), chunks[0]);

    let body_area = chunks[1];
    let body_width = body_area.width.saturating_sub(1) as usize; // room for scrollbar
    let content_len = state.details_mut().prepare_body_lines(body_width);
    let viewport = body_area.height as usize;
    state
        .details_mut()
        .sync_geometry(body_area, content_len, viewport);
    if body_area.height == 0 || content_len == 0 {
        return;
    }

    frame.render_widget(
        Paragraph::new(state.details().visible_body_lines()),
        body_area,
    );

    let now = Instant::now();
    if let Some(scrollbar) = state
        .details()
        .scrollbar()
        .filter(|_| state.details().should_render_scrollbar(now))
    {
        scrollbar.render(frame, state.details().dragging_scrollbar());
    }
}

fn detail_pane_heights(meta: &[Line<'_>], inner: Rect, has_body: bool) -> (u16, u16) {
    let width = inner.width.max(1) as usize;
    let wrapped_height = meta
        .iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            super::super::render::wrap_line_at_whitespace(&text, width).len()
        })
        .sum::<usize>()
        .min(inner.height as usize) as u16;
    let body_reserve = if has_body {
        MIN_DETAILS_BODY_ROWS.min(inner.height)
    } else {
        0
    };
    let meta_height = wrapped_height.min(inner.height.saturating_sub(body_reserve));
    (meta_height, inner.height.saturating_sub(meta_height))
}

fn detail_meta_lines<'a>(
    node: &'a WorkflowNodeSnapshot,
    state: &'a WorkflowUiState,
) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", state_glyph(&node.state)),
                state_style(&node.state),
            ),
            Span::styled(
                node.display_name.as_str(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(state_label(&node.state)),
        Line::from(kind_line(node)),
    ];

    if !node.work.is_empty() {
        lines.push(Line::from(format!("task {}", node.work)));
    }

    if matches!(node.access, WorkspaceAccess::Mutating) {
        lines.push(Line::from("writes to the workspace"));
    }

    let waiting = waiting_on(node, state);
    if !waiting.is_empty() {
        lines.push(Line::from(format!("waiting on {waiting}")));
    }

    if let Some(progress) = state.progress(node) {
        lines.push(Line::from(progress_now_line(progress)));
        if let Some(detail) = progress.detail.as_deref().filter(|value| !value.is_empty()) {
            lines.push(Line::from(format!("  {detail}")));
        }
    } else if matches!(node.state, NodeState::Running { .. }) {
        lines.push(Line::from("now starting…"));
    }

    if let Some(exit) = &node.command_exit {
        if !matches!(
            (&node.state, exit),
            (
                NodeState::Terminal {
                    outcome: NodeTerminalState::Success
                },
                CommandExit::Code { code: 0 }
            )
        ) {
            lines.push(Line::from(format!("exit {}", exit_label(exit))));
        }
    }

    if let Some(reason) = &node.terminal_reason {
        lines.push(Line::from(format!("because {}", reason_text(reason))));
    }

    if let Some(recovery) = state
        .snapshot()
        .recovery_requirement
        .as_ref()
        .filter(|recovery| recovery.node == node.id)
    {
        lines.push(Line::from(format!(
            "needs recovery · attempt {} may still own the process",
            recovery.attempt
        )));
    }

    // When the full body is available, skip the short one-line dump.
    if !state.details().has_body() {
        if let Some(output) = interesting_output(node) {
            lines.push(Line::from(format!("result {output}")));
        }
    }

    lines
}

fn node_graph_activity(node: &WorkflowNodeSnapshot, state: &WorkflowUiState) -> Option<String> {
    if let Some(progress) = state.progress(node) {
        return Some(progress.message.clone());
    }
    if matches!(node.state, NodeState::Running { .. }) {
        if node.work.is_empty() {
            return Some("working".into());
        }
        return Some(node.work.clone());
    }
    None
}

fn progress_now_line(progress: &super::event_adapter::WorkflowProgress) -> String {
    match (progress.completed, progress.total) {
        (Some(completed), Some(total)) if total > 0 => {
            format!("now {completed}/{total} · {}", progress.message)
        }
        (Some(completed), _) => format!("now turn {completed} · {}", progress.message),
        _ => format!("now {}", progress.message),
    }
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &WorkflowUiState) {
    let snapshot = state.snapshot();
    let policy = state.policy();
    let mut keys = vec!["j/k move".to_owned()];
    if state.details().is_scrollable() || state.details().has_body() {
        keys.push("pgup/pgdn scroll".into());
    }
    if matches!(
        state.session(),
        super::event_adapter::WorkflowSession::Watcher
    ) {
        keys.push("watch".into());
    }
    match policy.confirm {
        Some(ConfirmKind::StartPlan) => keys.push("enter start".into()),
        Some(ConfirmKind::ContinueResume) => keys.push("enter continue".into()),
        None => {}
    }
    // Stop hint is cancel capability minus an already-requested cancel.
    if policy.cancel_plain_c && !matches!(snapshot.cancellation, CancellationState::Requested) {
        keys.push("c stop".into());
    }
    if policy.show_leave_hint {
        keys.push("q leave".into());
    }
    if matches!(snapshot.cancellation, CancellationState::Requested) {
        keys.push("stopping…".into());
    }
    let mut text = keys.join("  ·  ");
    if let Some(notice) = state.notice() {
        text = format!("{notice}  ·  {text}");
    }
    frame.render_widget(Paragraph::new(text), area);
}

fn progress_summary(state: &WorkflowUiState) -> String {
    let total = state.snapshot().nodes.len();
    if total == 0 {
        return "0 steps".into();
    }
    let done = state
        .snapshot()
        .nodes
        .iter()
        .filter(|node| node.state.terminal().is_some())
        .count();
    let running = state
        .snapshot()
        .nodes
        .iter()
        .filter(|node| matches!(node.state, NodeState::Running { .. }))
        .collect::<Vec<_>>();
    if running.is_empty() {
        return format!("{done}/{total} done");
    }
    let focus = running
        .iter()
        .find_map(|node| {
            state
                .progress(node)
                .map(|progress| format!("{}: {}", node.display_name, progress.message))
        })
        .or_else(|| {
            running.first().map(|node| {
                if node.work.is_empty() {
                    format!("{} running", node.display_name)
                } else {
                    format!("{}: {}", node.display_name, short_work(&node.work))
                }
            })
        })
        .unwrap_or_else(|| format!("{} running", running.len()));
    format!("{done}/{total} done · {focus}")
}

fn short_work(work: &str) -> String {
    if work.chars().count() <= 48 {
        work.to_owned()
    } else {
        let mut out = work.chars().take(47).collect::<String>();
        out.push('…');
        out
    }
}

fn run_status_label(
    lifecycle: RunLifecycle,
    outcome: Option<WorkflowOutcome>,
    cancellation: CancellationState,
) -> String {
    if matches!(cancellation, CancellationState::Requested) {
        return "stopping".into();
    }
    match lifecycle {
        RunLifecycle::Planned => "ready".into(),
        RunLifecycle::Running => "running".into(),
        RunLifecycle::Cancelling => "stopping".into(),
        RunLifecycle::NeedsRecovery => "needs recovery".into(),
        RunLifecycle::Completed => match outcome {
            Some(WorkflowOutcome::Success) => "finished · success".into(),
            Some(WorkflowOutcome::Failure) => "finished · failed".into(),
            Some(WorkflowOutcome::Denial) => "finished · denied".into(),
            Some(WorkflowOutcome::Cancellation) => "finished · cancelled".into(),
            Some(WorkflowOutcome::Blocked) => "finished · blocked".into(),
            None => "finished".into(),
        },
    }
}

fn kind_line(node: &WorkflowNodeSnapshot) -> String {
    match &node.execution {
        ExecutionMetadata::Agent {
            name,
            provider,
            model,
            ..
        } => {
            let model = match (provider.as_deref(), model.as_deref()) {
                (Some(provider), Some(model)) => format!(" · {provider}/{model}"),
                (None, Some(model)) => format!(" · {model}"),
                _ => String::new(),
            };
            format!("agent {name}{model}")
        }
        ExecutionMetadata::Command {
            executable, shell, ..
        } => {
            let mode = if *shell { "shell" } else { "command" };
            let name = std::path::Path::new(executable)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(executable);
            format!("{mode} {name}")
        }
    }
}

fn waiting_on(node: &WorkflowNodeSnapshot, state: &WorkflowUiState) -> String {
    if !matches!(node.state, NodeState::Pending | NodeState::Ready) {
        return String::new();
    }
    let by_id = state
        .snapshot()
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<std::collections::BTreeMap<_, _>>();
    node.dependencies
        .iter()
        .filter_map(|dep| by_id.get(dep))
        .filter(|dep| {
            dep.state.terminal() != Some(NodeTerminalState::Success)
                && dep.state.terminal() != Some(NodeTerminalState::Skipped)
        })
        .map(|dep| dep.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn interesting_output(node: &WorkflowNodeSnapshot) -> Option<String> {
    let output = node.validated_output.as_ref()?;
    // Only show short results; full dumps belong in the scrollable body.
    let text = output.to_string();
    if text.chars().count() > 120 {
        return None;
    }
    if matches!(
        node.state,
        NodeState::Terminal {
            outcome: NodeTerminalState::Success | NodeTerminalState::Failure
        }
    ) {
        Some(text)
    } else {
        None
    }
}

fn exit_label(exit: &CommandExit) -> String {
    match exit {
        CommandExit::Code { code } => format!("code {code}"),
        CommandExit::Signal { signal } => format!("signal {signal}"),
        CommandExit::Timeout => "timeout".into(),
        CommandExit::Cancellation => "cancelled".into(),
        CommandExit::Abnormal => "abnormal stop".into(),
    }
}

fn reason_text(reason: &TerminalReason) -> &str {
    match reason {
        TerminalReason::Failure(reason)
        | TerminalReason::Denial(reason)
        | TerminalReason::Cancellation(reason)
        | TerminalReason::Blocked(reason) => reason,
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
