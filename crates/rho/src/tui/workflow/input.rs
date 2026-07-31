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
        KeyCode::Char('c') | KeyCode::Esc
            if !state.can_exit()
                && matches!(
                    state.lifecycle(),
                    RunLifecycle::Running | RunLifecycle::Cancelling
                ) =>
        {
            InputResult::Action(WorkflowAction::Cancel)
        }
        KeyCode::Char('c')
            if key.modifiers.contains(KeyModifiers::CONTROL) && !state.can_exit() =>
        {
            InputResult::Action(WorkflowAction::Cancel)
        }
        KeyCode::Char('q') | KeyCode::Esc if state.can_exit() => InputResult::Exit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) && state.can_exit() => {
            InputResult::Exit
        }
        _ => InputResult::Ignore,
    }
}
