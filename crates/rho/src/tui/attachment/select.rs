//! Standalone overlay used by `rho attach` when no run id is given.

use crossterm::event::Event;
use ratatui::DefaultTerminal;

use super::super::{
    attach_picker::{self, AttachCandidate, WorkspaceRunFilter},
    picker::runner::{self, EmptySubmit, EventHandling},
    terminal_events::TerminalEvents,
    UiPicker,
};

pub(super) async fn select_running_run(
    terminal: &mut DefaultTerminal,
    events: &mut TerminalEvents,
) -> anyhow::Result<Option<String>> {
    let cwd = std::env::current_dir()?;
    let candidates =
        tokio::task::spawn_blocking(move || attach_picker::workspace_candidates(&cwd)).await??;
    let mut filter = WorkspaceRunFilter::RunningOnly;
    let picker = attach_picker::picker(&candidates, filter);
    // No session config is loaded. Neither model key hint is enabled, so only
    // the defaults are consulted.
    let keybindings = crate::keybindings::Keybindings::default();
    runner::run(
        terminal,
        events,
        picker,
        &keybindings,
        EmptySubmit::Cancel,
        |picker, event| {
            if let Event::Key(key) = event {
                if attach_picker::is_running_filter_toggle(*key) {
                    filter = filter.toggled();
                    *picker = restore_picker(&candidates, filter, picker);
                    return EventHandling::Handled;
                }
            }
            EventHandling::Continue
        },
    )
    .await
}

fn restore_picker(
    candidates: &[AttachCandidate],
    filter: WorkspaceRunFilter,
    current: &UiPicker,
) -> UiPicker {
    let cursor = current.cursor();
    let mut next = attach_picker::picker(candidates, filter);
    next.restore_cursor(&cursor);
    next
}
