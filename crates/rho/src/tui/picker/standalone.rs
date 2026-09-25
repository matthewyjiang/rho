//! Terminal ownership for a picker without a conversation or provider.

use crossterm::event::Event;

use super::{
    runner::{self, EmptySubmit, EventHandling},
    UiPicker,
};
use crate::{
    keybindings::Keybindings,
    tui::{keyboard_modes, mouse_capture, terminal_events::TerminalEvents, Theme},
};

pub(in crate::tui) async fn select(
    picker: UiPicker,
    keybindings: &Keybindings,
    theme: &str,
) -> anyhow::Result<Option<String>> {
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal {
        keyboard: Some(keyboard_modes::Enabled::acquire()),
        mouse_capture: mouse_capture::Guard::acquire(),
    };
    Theme::initialize_from_terminal();
    Theme::apply_committed(theme);
    let mut events = TerminalEvents::new();
    runner::run(
        &mut terminal,
        &mut events,
        picker,
        keybindings,
        EmptySubmit::Stay,
        |picker, event| {
            if let Event::Paste(text) = event {
                for ch in text.chars().filter(|ch| !ch.is_control()) {
                    picker.push_filter_char(ch);
                }
                return EventHandling::Handled;
            }
            EventHandling::Continue
        },
    )
    .await
}

/// Undo terminal modes in reverse order of acquisition, then leave the
/// alternate screen.
struct RestoreTerminal {
    keyboard: Option<keyboard_modes::Enabled>,
    mouse_capture: mouse_capture::Guard,
}

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        self.mouse_capture.release();
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
        ratatui::restore();
    }
}
