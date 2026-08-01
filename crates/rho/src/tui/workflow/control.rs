//! Leave / cancel / confirm policy for the workflow screen.
//!
//! Input and footer both read this table so key hints cannot drift from keys.
//! Presentation-only labels (watch chrome, stop hint) are derived in the footer
//! from session + these fields so the policy table stays minimal.

use super::event_adapter::{PlanApprovalState, WorkflowSession, WorkflowSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfirmKind {
    StartPlan,
    ContinueResume,
}

/// Resolved key and footer policy for the current session and snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ControlPolicy {
    pub(super) can_leave: bool,
    pub(super) cancel_plain_c: bool,
    /// Esc / Ctrl-C request cancel instead of leave.
    pub(super) cancel_on_interrupt: bool,
    pub(super) confirm: Option<ConfirmKind>,
    /// Match prior footer: leave hint only after the plan is approved (owner)
    /// or always for watchers.
    pub(super) show_leave_hint: bool,
}

pub(super) fn control_policy(
    session: WorkflowSession,
    snapshot: &WorkflowSnapshot,
) -> ControlPolicy {
    let live = snapshot.lifecycle.is_live();
    let approved = matches!(snapshot.approval, PlanApprovalState::Approved);

    match session {
        WorkflowSession::Watcher => ControlPolicy {
            can_leave: true,
            cancel_plain_c: live && approved,
            cancel_on_interrupt: false,
            confirm: None,
            show_leave_hint: true,
        },
        WorkflowSession::Owner => ControlPolicy {
            // Owner leave tracks durable lifecycle only: not live ⇒ safe to leave.
            can_leave: !live,
            cancel_plain_c: live && approved,
            cancel_on_interrupt: live,
            confirm: match snapshot.approval {
                PlanApprovalState::AwaitingPlan => Some(ConfirmKind::StartPlan),
                PlanApprovalState::AwaitingResume => Some(ConfirmKind::ContinueResume),
                PlanApprovalState::Approved => None,
            },
            // Match prior footer: leave hint only after the plan is approved.
            show_leave_hint: !live && approved,
        },
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
