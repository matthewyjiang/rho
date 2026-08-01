use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::workflow::{
    ArtifactKind, CommandExit, NodeState, NodeTerminalState, RunLifecycle, WorkflowOutcome,
    WorkspaceAccess,
};

use super::{
    dag::{self, state_glyph, state_label, state_style},
    event_adapter::{
        CancellationState, ExecutionMetadata, PlanApprovalState, TerminalReason,
        WorkflowNodeSnapshot,
    },
    state::WorkflowUiState,
};

pub(super) fn draw(frame: &mut Frame<'_>, state: &WorkflowUiState) {
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

fn draw_header(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
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

fn draw_dag(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
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

fn draw_details(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
    let lines = state
        .selected_node()
        .map(|node| detail_lines(node, state))
        .unwrap_or_else(|| vec![Line::from("No steps")]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(" Selected ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn detail_lines<'a>(node: &'a WorkflowNodeSnapshot, state: &'a WorkflowUiState) -> Vec<Line<'a>> {
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

    if let Some(path) = primary_artifact_path(node) {
        lines.push(Line::from(format!("output {path}")));
    }

    // Keep structured output only when short and terminal-interesting.
    if let Some(output) = interesting_output(node) {
        lines.push(Line::from(format!("result {output}")));
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

fn draw_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
    let snapshot = state.snapshot();
    let mut keys = vec!["j/k move".to_owned()];
    if snapshot.detachable {
        keys.push("watch".into());
        if matches!(
            snapshot.lifecycle,
            RunLifecycle::Running | RunLifecycle::Cancelling
        ) && !matches!(snapshot.cancellation, CancellationState::Requested)
        {
            keys.push("c stop".into());
        }
        keys.push("q leave".into());
    } else {
        match snapshot.approval {
            PlanApprovalState::AwaitingPlan => keys.push("enter start".into()),
            PlanApprovalState::AwaitingResume => keys.push("enter continue".into()),
            PlanApprovalState::Approved
                if !snapshot.exit_is_safe
                    && matches!(
                        snapshot.lifecycle,
                        RunLifecycle::Running | RunLifecycle::Cancelling
                    ) =>
            {
                keys.push("c stop".into());
            }
            PlanApprovalState::Approved if snapshot.exit_is_safe => keys.push("q leave".into()),
            PlanApprovalState::Approved => {}
        }
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

fn primary_artifact_path(node: &WorkflowNodeSnapshot) -> Option<&str> {
    let preferred = [
        ArtifactKind::AgentAnswer,
        ArtifactKind::StructuredOutput,
        ArtifactKind::Stdout,
        ArtifactKind::Stderr,
    ];
    for kind in preferred {
        if let Some(artifact) = node.artifacts.iter().find(|item| item.kind == kind) {
            return Some(artifact.artifact.relative_path.as_str());
        }
    }
    None
}

fn interesting_output(node: &WorkflowNodeSnapshot) -> Option<String> {
    let output = node.validated_output.as_ref()?;
    // Only show short results; full dumps belong in artifacts.
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
