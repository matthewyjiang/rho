use pretty_assertions::assert_eq;

use super::{control_policy, ConfirmKind, ControlPolicy};
use crate::{
    tui::workflow::event_adapter::{
        CancellationState, PlanApprovalState, SourceDigestSummary, WorkflowSession,
        WorkflowSnapshot,
    },
    workflow::{Digest, PlanId, RunId, RunLifecycle},
};

fn snapshot(
    approval: PlanApprovalState,
    lifecycle: RunLifecycle,
    cancellation: CancellationState,
) -> WorkflowSnapshot {
    WorkflowSnapshot {
        workflow_name: "demo".into(),
        plan_id: PlanId::new(),
        run_id: Some(RunId::new()),
        graph_digest: Digest("sha256:aa".into()),
        sources: SourceDigestSummary {
            source_count: 1,
            digest: Digest("sha256:bb".into()),
        },
        approval,
        lifecycle,
        outcome: None,
        nodes: Vec::new(),
        cancellation,
        recovery_requirement: None,
    }
}

// Covers: owner vs watcher leave/cancel matrix must stay one policy table.
// Owner: workflow TUI control policy.
#[test]
fn control_policy_matrix() {
    let cases = [
        (
            "owner live",
            WorkflowSession::Owner,
            snapshot(
                PlanApprovalState::Approved,
                RunLifecycle::Running,
                CancellationState::NotRequested,
            ),
            ControlPolicy {
                can_leave: false,
                cancel_plain_c: true,
                cancel_on_interrupt: true,
                confirm: None,
                show_leave_hint: false,
            },
        ),
        (
            "owner durable",
            WorkflowSession::Owner,
            snapshot(
                PlanApprovalState::Approved,
                RunLifecycle::Completed,
                CancellationState::NotRequested,
            ),
            ControlPolicy {
                can_leave: true,
                cancel_plain_c: false,
                cancel_on_interrupt: false,
                confirm: None,
                show_leave_hint: true,
            },
        ),
        (
            "owner awaiting plan",
            WorkflowSession::Owner,
            snapshot(
                PlanApprovalState::AwaitingPlan,
                RunLifecycle::Planned,
                CancellationState::NotRequested,
            ),
            ControlPolicy {
                can_leave: true,
                cancel_plain_c: false,
                cancel_on_interrupt: false,
                confirm: Some(ConfirmKind::StartPlan),
                show_leave_hint: false,
            },
        ),
        (
            "watcher live",
            WorkflowSession::Watcher,
            snapshot(
                PlanApprovalState::Approved,
                RunLifecycle::Running,
                CancellationState::NotRequested,
            ),
            ControlPolicy {
                can_leave: true,
                cancel_plain_c: true,
                cancel_on_interrupt: false,
                confirm: None,
                show_leave_hint: true,
            },
        ),
        (
            "watcher live cancel requested",
            WorkflowSession::Watcher,
            snapshot(
                PlanApprovalState::Approved,
                RunLifecycle::Cancelling,
                CancellationState::Requested,
            ),
            ControlPolicy {
                can_leave: true,
                cancel_plain_c: true,
                cancel_on_interrupt: false,
                confirm: None,
                show_leave_hint: true,
            },
        ),
        (
            "watcher done",
            WorkflowSession::Watcher,
            snapshot(
                PlanApprovalState::Approved,
                RunLifecycle::Completed,
                CancellationState::NotRequested,
            ),
            ControlPolicy {
                can_leave: true,
                cancel_plain_c: false,
                cancel_on_interrupt: false,
                confirm: None,
                show_leave_hint: true,
            },
        ),
    ];

    for (label, session, snap, expected) in cases {
        assert_eq!(control_policy(session, &snap), expected, "{label}");
    }
}
