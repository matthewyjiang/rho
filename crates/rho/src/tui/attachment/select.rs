//! Standalone overlay used by `rho attach` when no run id is given.

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{widgets::Clear, DefaultTerminal, Frame};

use crate::subagent;

use super::super::{
    attach_picker::{self, AttachCandidate},
    picker_input::{apply_picker_key, overlay_scroll_targets, PickerKeyEffect},
    picker_overlay::picker_overlay_frame,
    Theme, UiPicker,
};

pub(super) async fn select_running_run(
    terminal: &mut DefaultTerminal,
) -> anyhow::Result<Option<String>> {
    let candidates = tokio::task::spawn_blocking(subagent::list_running_runs).await??;
    if candidates.is_empty() {
        anyhow::bail!("no running subagents");
    }
    let candidates = candidates
        .into_iter()
        .map(AttachCandidate::from)
        .collect::<Vec<_>>();
    let mut picker = attach_picker::picker(&candidates);
    loop {
        terminal.draw(|frame| draw_picker(frame, &picker))?;
        match next_event().await? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(None);
                }
                let targets = overlay_scroll_targets(&picker, terminal);
                match apply_picker_key(&mut picker, key, targets, /*space_confirms*/ false) {
                    PickerKeyEffect::Submit => {
                        return Ok(picker.selected_item().map(|item| item.value.clone()));
                    }
                    PickerKeyEffect::Escape => return Ok(None),
                    PickerKeyEffect::Handled
                    | PickerKeyEffect::None
                    | PickerKeyEffect::ToggleFavorite
                    | PickerKeyEffect::DeleteRow => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
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
