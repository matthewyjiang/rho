//! Picker rendering and navigation on a caller-owned terminal and event stream.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    widgets::{Clear, Paragraph},
    DefaultTerminal,
};

use super::{
    apply_picker_key, overlay_scroll_targets, picker_overlay_frame, PickerKeyEffect, UiPicker,
};
use crate::{
    keybindings::Keybindings,
    tui::{terminal_events::TerminalEvents, Theme},
};

pub(in crate::tui) enum EmptySubmit {
    Stay,
    Cancel,
}

pub(in crate::tui) enum EventHandling {
    Handled,
    Continue,
}

/// The feature handler runs before navigation and owns special keys and paste.
/// Reuse the caller's stream when continuing into another terminal UI.
pub(in crate::tui) async fn run(
    terminal: &mut DefaultTerminal,
    events: &mut TerminalEvents,
    mut picker: UiPicker,
    keybindings: &Keybindings,
    empty_submit: EmptySubmit,
    mut handle_event: impl FnMut(&mut UiPicker, &Event) -> EventHandling,
) -> anyhow::Result<Option<String>> {
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Clear, area);
            if let Some(overlay) = picker_overlay_frame(&picker, area) {
                frame.render_widget(
                    Paragraph::new(overlay.lines).style(Theme::text()),
                    overlay.outer,
                );
                frame.set_cursor_position(overlay.cursor);
            }
        })?;
        let event = events.next().await?;
        if let Event::Key(key) = &event {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(None);
            }
        }
        if matches!(handle_event(&mut picker, &event), EventHandling::Handled) {
            continue;
        }
        if let Event::Key(key) = event {
            let targets = overlay_scroll_targets(&picker, terminal);
            match apply_picker_key(
                &mut picker,
                key,
                targets,
                /*space_confirms*/ false,
                keybindings,
            ) {
                PickerKeyEffect::Submit => {
                    if let Some(item) = picker.selected_item() {
                        return Ok(Some(item.value.clone()));
                    }
                    match empty_submit {
                        EmptySubmit::Stay => {}
                        EmptySubmit::Cancel => return Ok(None),
                    }
                }
                PickerKeyEffect::Escape => return Ok(None),
                PickerKeyEffect::None
                | PickerKeyEffect::Handled
                | PickerKeyEffect::ToggleFavorite
                | PickerKeyEffect::ToggleModelScope
                | PickerKeyEffect::DeleteRow => {}
            }
        }
    }
}
