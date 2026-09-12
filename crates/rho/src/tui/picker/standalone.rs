//! Run a picker without constructing a conversation or starting a provider.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::widgets::{Clear, Paragraph};

use super::{
    apply_picker_key, overlay_scroll_targets, picker_overlay_frame, PickerKeyEffect, UiPicker,
};
use crate::{
    keybindings::Keybindings,
    tui::{keyboard_modes, terminal_events::TerminalEvents, Theme},
};

/// The caller supplies rows and policy. Only navigation, filtering, and confirmation
/// are enabled here; feature actions such as pinning need their own event loop.
pub(in crate::tui) async fn select(
    mut picker: UiPicker,
    keybindings: &Keybindings,
    theme: &str,
) -> anyhow::Result<Option<String>> {
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal(Some(keyboard_modes::Enabled::acquire()));
    Theme::initialize_from_terminal();
    Theme::apply_committed(theme);
    let mut events = TerminalEvents::new();
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
        if let Event::Paste(text) = &event {
            for ch in text.chars().filter(|ch| !ch.is_control()) {
                picker.push_filter_char(ch);
            }
        }
        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(None);
            }
            let targets = overlay_scroll_targets(&picker, &terminal);
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

struct RestoreTerminal(Option<keyboard_modes::Enabled>);

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        if let Some(keyboard) = self.0.take() {
            keyboard.release();
        }
        ratatui::restore();
    }
}
