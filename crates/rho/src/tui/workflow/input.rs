use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{
    control::ConfirmKind, dag::HorizontalDirection, event_adapter::WorkflowAction,
    state::WorkflowUiState,
};

#[derive(Debug)]
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
        KeyCode::Up | KeyCode::Char('k') if !ctrl => {
            state.select_previous();
            InputResult::Redraw
        }
        KeyCode::Down | KeyCode::Char('j') if !ctrl => {
            state.select_next();
            InputResult::Redraw
        }
        KeyCode::Left | KeyCode::Char('h') if !ctrl => {
            state.select_horizontal(HorizontalDirection::Left);
            InputResult::Redraw
        }
        KeyCode::Right | KeyCode::Char('l') if !ctrl => {
            state.select_horizontal(HorizontalDirection::Right);
            InputResult::Redraw
        }
        KeyCode::PageUp => scroll_details(state, ScrollCommand::PageUp),
        KeyCode::PageDown => scroll_details(state, ScrollCommand::PageDown),
        KeyCode::Char('u') if ctrl => scroll_details(state, ScrollCommand::PageUp),
        KeyCode::Char('d') if ctrl => scroll_details(state, ScrollCommand::PageDown),
        KeyCode::Home => scroll_details(state, ScrollCommand::Home),
        KeyCode::End => scroll_details(state, ScrollCommand::End),
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

enum ScrollCommand {
    PageUp,
    PageDown,
    Home,
    End,
}

fn scroll_details(state: &mut WorkflowUiState, command: ScrollCommand) -> InputResult {
    let details = state.details_mut();
    match command {
        ScrollCommand::PageUp => details.scroll_page(-1),
        ScrollCommand::PageDown => details.scroll_page(1),
        ScrollCommand::Home => details.scroll_home(),
        ScrollCommand::End => details.scroll_end(),
    }
    details.reveal_scrollbar(Instant::now());
    InputResult::Redraw
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
