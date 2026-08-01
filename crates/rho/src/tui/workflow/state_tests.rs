use pretty_assertions::assert_eq;

use super::WorkflowUiState;
use crate::{
    tui::workflow::event_adapter::{
        CancellationState, ExecutionMetadata, PlanApprovalState, SourceDigestSummary,
        WorkflowEvent, WorkflowNodeSnapshot, WorkflowProgress, WorkflowSession, WorkflowSnapshot,
    },
    workflow::{
        AgentRuntime, AttemptNumber, Digest, NodeId, NodeState, PlanId, RunId, RunLifecycle,
        WorkspaceAccess,
    },
};

fn snapshot(nodes: Vec<WorkflowNodeSnapshot>) -> WorkflowSnapshot {
    WorkflowSnapshot {
        workflow_name: "demo".into(),
        plan_id: PlanId::new(),
        run_id: Some(RunId::new()),
        graph_digest: Digest("sha256:aa".into()),
        sources: SourceDigestSummary {
            source_count: 1,
            digest: Digest("sha256:bb".into()),
        },
        approval: PlanApprovalState::Approved,
        lifecycle: RunLifecycle::Running,
        outcome: None,
        nodes,
        cancellation: CancellationState::NotRequested,
        recovery_requirement: None,
        exit_is_safe: false,
    }
}

fn running_node(id: &str, name: &str, work: &str) -> WorkflowNodeSnapshot {
    let attempt = AttemptNumber::new(1).unwrap();
    WorkflowNodeSnapshot {
        id: NodeId::new(id).unwrap(),
        display_name: name.into(),
        dependencies: Vec::new(),
        access: WorkspaceAccess::ReadOnly,
        execution: ExecutionMetadata::Agent {
            name: "reviewer".into(),
            runtime: AgentRuntime::Rho,
            provider: None,
            model: None,
        },
        work: work.into(),
        state: NodeState::Running { attempt },
        current_attempt: Some(attempt),
        command_exit: None,
        validated_output: None,
        artifacts: Vec::new(),
        terminal_reason: None,
    }
}

// Covers: live progress attaches to the matching running attempt.
// Owner: workflow run TUI state.
#[test]
fn progress_is_visible_for_current_attempt() {
    let node = running_node("review", "Review", "audit the change");
    let attempt = node.current_attempt.unwrap();
    let mut state = WorkflowUiState::new(WorkflowSession::Owner, snapshot(vec![node]));
    state.apply(WorkflowEvent::Progress {
        node: NodeId::new("review").unwrap(),
        progress: WorkflowProgress {
            attempt,
            completed: Some(3),
            total: None,
            message: "tool: Bash".into(),
            detail: Some("git diff main".into()),
        },
    });
    let selected = state.selected_node().unwrap();
    let progress = state.progress(selected).unwrap();
    assert_eq!(progress.message, "tool: Bash");
    assert_eq!(progress.detail.as_deref(), Some("git diff main"));
    assert_eq!(progress.completed, Some(3));
}
