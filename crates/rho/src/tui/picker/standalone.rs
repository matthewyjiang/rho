//! Terminal ownership for a picker without a conversation or provider.

use super::{runner, UiPicker};
use crate::{
    keybindings::Keybindings,
    tui::{keyboard_modes, mouse_capture, Theme},
};

pub(in crate::tui) async fn select(
    picker: UiPicker,
    keybindings: &Keybindings,
    theme: &str,
) -> anyhow::Result<Option<String>> {
    let mut terminal = crate::tui::synced_backend::try_init()?;
    let _restore = RestoreTerminal {
        keyboard: Some(keyboard_modes::Enabled::acquire()),
        mouse_capture: mouse_capture::Guard::acquire(),
    };
    Theme::initialize_from_terminal();
    Theme::apply_committed(theme);
    runner::run(&mut terminal, picker, keybindings).await
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
