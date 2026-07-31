use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::workflow::{AgentRuntime, CommandExit, NodeState, NodeTerminalState, WorkspaceAccess};

use super::{
    event_adapter::{
        ArtifactKind, CancellationState, ExecutionMetadata, PlanApprovalState, RecoveryRequirement,
        TerminalReason, WorkflowNodeSnapshot,
    },
    state::WorkflowUiState,
};

pub(super) fn draw(frame: &mut Frame<'_>, state: &WorkflowUiState) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, vertical[0], state);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
        .split(vertical[1]);
    draw_nodes(frame, body[0], state);
    draw_details(frame, body[1], state);
    draw_footer(frame, vertical[2], state);
}

fn draw_header(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
    let snapshot = state.snapshot();
    let counts = state.counts();
    let run = snapshot
        .run_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "not created".into());
    let lines = vec![
        Line::from(vec![
            Span::styled("Workflow", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "  {:?}  outcome: {}  approval: {}",
                snapshot.lifecycle,
                snapshot
                    .outcome
                    .map(|outcome| format!("{outcome:?}"))
                    .unwrap_or_else(|| "pending".into()),
                approval(snapshot.approval)
            )),
        ]),
        Line::from(format!("plan: {}", snapshot.plan_id)),
        Line::from(format!("run: {run}")),
        Line::from(format!(
            "graph: {}  sources: {} ({})",
            short_digest(&snapshot.graph_digest.0),
            snapshot.sources.source_count,
            short_digest(&snapshot.sources.digest.0),
        )),
        Line::from(format!(
            "states  pending:{} ready:{} running:{} success:{} failure:{} denied:{} cancelled:{} skipped:{} blocked:{}",
            counts.pending,
            counts.ready,
            counts.running,
            counts.success,
            counts.failure,
            counts.denial,
            counts.cancelled,
            counts.skipped,
            counts.blocked,
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_nodes(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
    let items = state
        .snapshot()
        .nodes
        .iter()
        .map(|node| {
            let access = match node.access {
                WorkspaceAccess::ReadOnly => "read only",
                WorkspaceAccess::Mutating => "mutating",
            };
            let attempt = node
                .current_attempt
                .map(|attempt| format!(" attempt {attempt}"))
                .unwrap_or_default();
            let line = Line::from(vec![
                Span::styled(
                    node.id.to_string(),
                    state_style(&node.state).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    " [{}] {access}{attempt}",
                    node_state_label(&node.state)
                )),
            ]);
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    let mut list_state = ListState::default().with_selected(Some(state.selected_index()));
    frame.render_stateful_widget(
        List::new(items).highlight_symbol("▶ ").block(
            Block::default()
                .title(" Nodes - scheduler order ")
                .borders(Borders::ALL),
        ),
        area,
        &mut list_state,
    );
}

fn draw_details(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
    let lines = state
        .selected_node()
        .map(|node| detail_lines(node, state))
        .unwrap_or_else(|| vec![Line::from("No workflow nodes")]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Node details ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn detail_lines<'a>(node: &'a WorkflowNodeSnapshot, state: &'a WorkflowUiState) -> Vec<Line<'a>> {
    let dependencies = if node.dependencies.is_empty() {
        "none".into()
    } else {
        node.dependencies
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines = vec![
        Line::from(Span::styled(
            node.display_name.as_str(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("id: {}", node.id)),
        Line::from(format!("dependencies: {dependencies}")),
        Line::from(format!("access: {}", access_label(node.access))),
        Line::from(execution_label(&node.execution)),
    ];
    if let Some(progress) = state.progress(node) {
        lines.push(Line::from(format!(
            "progress: {}/{} {}",
            progress.completed, progress.total, progress.message
        )));
    }
    if let Some(exit) = &node.command_exit {
        lines.push(Line::from(format!("command exit: {}", exit_label(exit))));
    }
    if let Some(output) = &node.validated_output {
        let output = serde_json::to_string(output).unwrap_or_else(|_| "<invalid>".into());
        lines.push(Line::from(format!("validated output: {output}")));
    }
    for artifact in &node.artifacts {
        lines.push(Line::from(format!(
            "artifact {}: {} ({} bytes, {})",
            artifact_kind_label(&artifact.kind),
            artifact.artifact.relative_path,
            artifact.artifact.bytes,
            short_digest(&artifact.artifact.digest.0),
        )));
    }
    if let Some(reason) = &node.terminal_reason {
        lines.push(Line::from(format!("reason: {}", reason_label(reason))));
    }
    if let Some(recovery) = state
        .snapshot()
        .recovery_requirement
        .as_ref()
        .filter(|recovery| recovery.node == node.id)
    {
        lines.push(Line::from(recovery_label(recovery)));
    }
    lines
}

fn draw_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &WorkflowUiState) {
    let snapshot = state.snapshot();
    let action = match snapshot.approval {
        PlanApprovalState::AwaitingPlan => "Enter confirm plan  ↑/↓ nodes  q exit",
        PlanApprovalState::AwaitingResume => "Enter confirm resume  ↑/↓ nodes  q exit",
        PlanApprovalState::Approved if !snapshot.exit_is_safe => "↑/↓ nodes  c/Esc cancel and save",
        PlanApprovalState::Approved => "↑/↓ nodes  q/Esc exit",
    };
    let cancellation = match snapshot.cancellation {
        CancellationState::NotRequested => "not requested",
        CancellationState::Requested => "stopping active work",
        CancellationState::Saved => "saved and resumable",
    };
    let action = state
        .notice()
        .map_or_else(|| action.to_owned(), |notice| format!("{notice}  {action}"));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("cancellation: {cancellation}")),
            Line::from(action),
        ])
        .block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn approval(value: PlanApprovalState) -> &'static str {
    match value {
        PlanApprovalState::AwaitingPlan => "awaiting plan confirmation",
        PlanApprovalState::AwaitingResume => "awaiting resume confirmation",
        PlanApprovalState::Approved => "approved",
    }
}

fn node_state_label(state: &NodeState) -> &'static str {
    match state {
        NodeState::Pending => "pending",
        NodeState::Ready => "ready",
        NodeState::Running { .. } => "running",
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => "success",
            NodeTerminalState::Failure => "failure",
            NodeTerminalState::Denial => "denied",
            NodeTerminalState::Cancellation => "cancelled",
            NodeTerminalState::Skipped => "skipped",
            NodeTerminalState::Blocked => "blocked",
        },
    }
}

fn state_style(state: &NodeState) -> Style {
    let color = match state {
        NodeState::Pending | NodeState::Ready => Color::Gray,
        NodeState::Running { .. } => Color::Cyan,
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => Color::Green,
            NodeTerminalState::Skipped => Color::Yellow,
            NodeTerminalState::Failure
            | NodeTerminalState::Denial
            | NodeTerminalState::Cancellation
            | NodeTerminalState::Blocked => Color::Red,
        },
    };
    Style::default().fg(color)
}

fn access_label(access: WorkspaceAccess) -> &'static str {
    match access {
        WorkspaceAccess::ReadOnly => "read only",
        WorkspaceAccess::Mutating => "mutating exclusive",
    }
}

fn execution_label(metadata: &ExecutionMetadata) -> String {
    match metadata {
        ExecutionMetadata::Agent {
            name,
            runtime,
            provider,
            model,
        } => format!(
            "agent: {name} ({}) provider:{} model:{}",
            match runtime {
                AgentRuntime::Rho => "rho",
                AgentRuntime::ClaudeCli => "claude-cli",
            },
            provider.as_deref().unwrap_or("default"),
            model.as_deref().unwrap_or("default"),
        ),
        ExecutionMetadata::Command {
            executable,
            cwd,
            shell,
        } => format!(
            "command: {executable} cwd:{cwd} mode:{}",
            if *shell { "shell" } else { "direct" }
        ),
    }
}

fn exit_label(exit: &CommandExit) -> String {
    match exit {
        CommandExit::Code { code } => format!("code {code}"),
        CommandExit::Signal { signal } => format!("signal {signal}"),
        CommandExit::Timeout => "timeout".into(),
        CommandExit::Cancellation => "cancelled".into(),
        CommandExit::Abnormal => "abnormal termination".into(),
    }
}

fn reason_label(reason: &TerminalReason) -> &str {
    match reason {
        TerminalReason::Failure(reason)
        | TerminalReason::Denial(reason)
        | TerminalReason::Cancellation(reason)
        | TerminalReason::Blocked(reason) => reason,
    }
}

fn artifact_kind_label(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Stdout => "stdout",
        ArtifactKind::Stderr => "stderr",
        ArtifactKind::StructuredOutput => "structured output",
        ArtifactKind::AgentAnswer => "agent answer",
    }
}

fn recovery_label(recovery: &RecoveryRequirement) -> String {
    format!(
        "recovery required: node {} attempt {} has uncertain owner {:?}",
        recovery.node, recovery.attempt, recovery.uncertain_owner
    )
}

fn short_digest(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}
