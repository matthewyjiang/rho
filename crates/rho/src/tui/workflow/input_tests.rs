use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{handle_key, InputResult};
use crate::{
    tui::workflow::{
        event_adapter::{
            CancellationState, PlanApprovalState, SourceDigestSummary, WorkflowAction,
            WorkflowSession, WorkflowSnapshot,
        },
        state::WorkflowUiState,
    },
    workflow::{Digest, PlanId, RunId, RunLifecycle},
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn live_state(session: WorkflowSession) -> WorkflowUiState {
    WorkflowUiState::new(
        session,
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
            nodes: Vec::new(),
            cancellation: CancellationState::NotRequested,
            recovery_requirement: None,
        },
        /*run_directory*/ None,
    )
}

// Covers: watcher leave keys must exit while live; owner Esc/c must cancel.
// Owner: workflow TUI input vs session policy.
#[test]
fn watcher_leave_keys_do_not_cancel_while_owner_does() {
    let mut watcher = live_state(WorkflowSession::Watcher);
    assert!(matches!(
        handle_key(&mut watcher, key(KeyCode::Char('q'))),
        InputResult::Exit
    ));
    assert!(matches!(
        handle_key(&mut watcher, key(KeyCode::Esc)),
        InputResult::Exit
    ));
    assert!(matches!(
        handle_key(&mut watcher, ctrl_c()),
        InputResult::Exit
    ));
    assert_eq!(
        match handle_key(&mut watcher, key(KeyCode::Char('c'))) {
            InputResult::Action(action) => action,
            other => panic!("expected cancel action, got {other:?}"),
        },
        WorkflowAction::Cancel
    );

    let mut owner = live_state(WorkflowSession::Owner);
    assert!(matches!(
        handle_key(&mut owner, key(KeyCode::Char('q'))),
        InputResult::Ignore
    ));
    assert_eq!(
        match handle_key(&mut owner, key(KeyCode::Esc)) {
            InputResult::Action(action) => action,
            other => panic!("expected cancel action, got {other:?}"),
        },
        WorkflowAction::Cancel
    );
    assert_eq!(
        match handle_key(&mut owner, key(KeyCode::Char('c'))) {
            InputResult::Action(action) => action,
            other => panic!("expected cancel action, got {other:?}"),
        },
        WorkflowAction::Cancel
    );
    assert_eq!(
        match handle_key(&mut owner, ctrl_c()) {
            InputResult::Action(action) => action,
            other => panic!("expected cancel action, got {other:?}"),
        },
        WorkflowAction::Cancel
    );
}
