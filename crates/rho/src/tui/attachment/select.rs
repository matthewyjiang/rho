//! Standalone overlay used by `rho attach` when no run id is given.

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{widgets::Clear, DefaultTerminal, Frame};

use super::super::{
    attach_picker::{self, AttachCandidate, WorkspaceRunFilter},
    picker::picker_overlay_frame,
    picker::{apply_picker_key, overlay_scroll_targets, PickerKeyEffect},
    Theme, UiPicker,
};

pub(super) async fn select_running_run(
    terminal: &mut DefaultTerminal,
) -> anyhow::Result<Option<String>> {
    let cwd = std::env::current_dir()?;
    let candidates =
        tokio::task::spawn_blocking(move || attach_picker::workspace_candidates(&cwd)).await??;
    let mut filter = WorkspaceRunFilter::RunningOnly;
    let mut picker = attach_picker::picker(&candidates, filter);
    // Standalone overlay with no session config loaded. The attach picker
    // enables neither model key hint, so only the defaults are ever consulted.
    let keybindings = crate::keybindings::Keybindings::default();
    loop {
        terminal.draw(|frame| draw_picker(frame, &picker))?;
        match next_event().await? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(None);
                }
                if attach_picker::is_running_filter_toggle(key) {
                    filter = filter.toggled();
                    picker = restore_picker(&candidates, filter, &picker);
                    continue;
                }
                let targets = overlay_scroll_targets(&picker, terminal);
                match apply_picker_key(
                    &mut picker,
                    key,
                    targets,
                    /*space_confirms*/ false,
                    &keybindings,
                ) {
                    PickerKeyEffect::Submit => {
                        return Ok(picker.selected_item().map(|item| item.value.clone()));
                    }
                    PickerKeyEffect::Escape => return Ok(None),
                    PickerKeyEffect::Handled
                    | PickerKeyEffect::None
                    | PickerKeyEffect::ToggleFavorite
                    | PickerKeyEffect::ToggleModelScope
                    | PickerKeyEffect::DeleteRow => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
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

fn draw_picker(frame: &mut Frame<'_>, picker: &UiPicker) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    if let Some(overlay) = picker_overlay_frame(picker, area) {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(overlay.lines).style(Theme::text()),
            overlay.outer,
        );
        frame.set_cursor_position(overlay.cursor);
    }
}

async fn next_event() -> anyhow::Result<Event> {
    Ok(tokio::task::spawn_blocking(event::read).await??)
}
