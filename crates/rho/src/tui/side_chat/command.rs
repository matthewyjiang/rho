/// What `/side` / `/btw` should do given overlay state and args.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SideCommandAction {
    /// Empty `/side` while the overlay is closed.
    Open,
    /// Empty `/side` while the overlay is already open.
    ToggleClose,
    /// `/side <prompt>` (opens first when the overlay is closed).
    Submit(String),
}

pub(super) fn side_command_action(overlay_open: bool, args: &str) -> SideCommandAction {
    match (overlay_open, args.trim()) {
        (true, "") => SideCommandAction::ToggleClose,
        (false, "") => SideCommandAction::Open,
        (_, prompt) => SideCommandAction::Submit(prompt.to_string()),
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
