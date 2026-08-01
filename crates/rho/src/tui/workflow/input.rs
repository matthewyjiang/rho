use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{control::ConfirmKind, event_adapter::WorkflowAction, state::WorkflowUiState};

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

    let policy = state.policy();
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.select_previous();
            InputResult::Redraw
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.select_next();
            InputResult::Redraw
        }
        KeyCode::Enter => match policy.confirm {
            Some(ConfirmKind::StartPlan) => InputResult::Action(WorkflowAction::ConfirmPlan),
            Some(ConfirmKind::ContinueResume) => InputResult::Action(WorkflowAction::ConfirmResume),
            None => InputResult::Ignore,
        },
        KeyCode::Char('q') if policy.can_leave => InputResult::Exit,
        KeyCode::Esc if policy.can_leave => InputResult::Exit,
        KeyCode::Char('c') if ctrl && policy.can_leave => InputResult::Exit,
        KeyCode::Char('c') if !ctrl && policy.cancel_plain_c => {
            InputResult::Action(WorkflowAction::Cancel)
        }
        KeyCode::Char('c') if ctrl && policy.cancel_on_interrupt => {
            InputResult::Action(WorkflowAction::Cancel)
        }
        KeyCode::Esc if policy.cancel_on_interrupt => InputResult::Action(WorkflowAction::Cancel),
        _ => InputResult::Ignore,
    }
}
