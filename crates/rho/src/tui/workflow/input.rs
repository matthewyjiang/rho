use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::workflow::RunLifecycle;

use super::{event_adapter::WorkflowAction, state::WorkflowUiState};

pub(super) enum InputResult {
    Ignore,
    Redraw,
    Action(WorkflowAction),
    Exit,
}

pub(super) fn handle_key(state: &mut WorkflowUiState, key: KeyEvent) -> InputResult {
    if key.kind != KeyEventKind::Press {
        return InputResult::Ignore;
    }

    let lifecycle = state.lifecycle();
    let detachable = state.snapshot().detachable;
    let running = matches!(lifecycle, RunLifecycle::Running | RunLifecycle::Cancelling);

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.select_previous();
            InputResult::Redraw
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.select_next();
            InputResult::Redraw
        }
        KeyCode::Enter => match state.approval() {
            super::PlanApprovalState::AwaitingPlan => {
                InputResult::Action(WorkflowAction::ConfirmPlan)
            }
            super::PlanApprovalState::AwaitingResume => {
                InputResult::Action(WorkflowAction::ConfirmResume)
            }
            super::PlanApprovalState::Approved => InputResult::Ignore,
        },
        // Watch mode: leave with Esc/q; cancel only with plain `c`.
        KeyCode::Esc if detachable || state.can_exit() => InputResult::Exit,
        KeyCode::Char('q') if detachable || state.can_exit() => InputResult::Exit,
        KeyCode::Char('c')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && (detachable || state.can_exit()) =>
        {
            InputResult::Exit
        }
        KeyCode::Char('c')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(state.approval(), super::PlanApprovalState::Approved)
                && running =>
        {
            InputResult::Action(WorkflowAction::Cancel)
        }
        KeyCode::Char('c')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !state.can_exit()
                && !detachable =>
        {
            InputResult::Action(WorkflowAction::Cancel)
        }
        KeyCode::Esc if !state.can_exit() && !detachable && running => {
            InputResult::Action(WorkflowAction::Cancel)
        }
        _ => InputResult::Ignore,
    }
}
