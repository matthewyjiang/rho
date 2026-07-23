use std::io::Write;

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};

/// xterm modifyOtherKeys mode 2: report modified function keys (including
/// modified Enter) without altering ordinary shifted printable characters.
const ENABLE_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4;2m";
const DISABLE_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4;0m";

/// Session-scoped keyboard terminal protocols armed for the TUI lifetime.
///
/// Call [`Enabled::release`] during shutdown (before `ratatui::restore`) so
/// teardown order stays explicit at the call site.
pub(super) struct Enabled {
    bracketed_paste: bool,
    modified_keys: bool,
    keyboard_enhancements: bool,
}

impl Enabled {
    pub(super) fn acquire() -> Self {
        let extended = should_request_extended_keyboard_protocols();
        Self {
            bracketed_paste: enable_bracketed_paste().is_ok(),
            // Extended protocols share one Windows/ConPTY policy: see
            // should_request_extended_keyboard_protocols.
            modified_keys: extended && enable_modified_keys().is_ok(),
            keyboard_enhancements: extended && enable_keyboard_enhancements().is_ok(),
        }
    }

    pub(super) fn release(self) {
        let _ = self.try_release();
    }

    pub(super) fn try_release(self) -> std::io::Result<()> {
        let mut first_error = None;
        if self.keyboard_enhancements {
            record_first_error(&mut first_error, disable_keyboard_enhancements());
        }
        if self.modified_keys {
            record_first_error(&mut first_error, disable_modified_keys());
        }
        if self.bracketed_paste {
            record_first_error(&mut first_error, disable_bracketed_paste());
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn record_first_error(first_error: &mut Option<std::io::Error>, result: std::io::Result<()>) {
    if let Err(error) = result {
        first_error.get_or_insert(error);
    }
}

fn enable_bracketed_paste() -> std::io::Result<()> {
    execute!(std::io::stdout(), EnableBracketedPaste)
}

fn disable_bracketed_paste() -> std::io::Result<()> {
    execute!(std::io::stdout(), DisableBracketedPaste)
}

fn enable_keyboard_enhancements() -> std::io::Result<()> {
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
}

fn disable_keyboard_enhancements() -> std::io::Result<()> {
    execute!(std::io::stdout(), PopKeyboardEnhancementFlags)
}

fn enable_modified_keys() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(ENABLE_MODIFY_OTHER_KEYS)?;
    stdout.flush()
}

fn disable_modified_keys() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(DISABLE_MODIFY_OTHER_KEYS)?;
    stdout.flush()
}

/// Whether Rho should request Kitty keyboard enhancements and xterm
/// modifyOtherKeys.
///
/// Windows ConPTY reverse-maps only a small set of legacy key sequences into
/// KEY_EVENT records. Multiplexers such as Herdr re-encode keys for the pane
/// based on protocols the child requests:
/// - Kitty enhancements turn Shift+Tab into CSI u (`\x1b[9;2u`)
/// - xterm modifyOtherKeys mode 2 turns it into CSI 27 (`\x1b[27;2;9~`)
///
/// Neither form is reverse-translated by ConPTY, so Shift+Tab never reaches
/// Rho as BackTab. Legacy `\x1b[Z` is reverse-mapped to VK_TAB+SHIFT and works.
///
/// Both extended protocols are gated together on purpose: under ConPTY+Herdr
/// we prefer reliable Shift+Tab (permission-mode cycle) over the extra
/// modified-Enter fidelity those protocols provide on other platforms.
pub(super) fn should_request_extended_keyboard_protocols() -> bool {
    !cfg!(windows)
}

#[cfg(test)]
#[path = "keyboard_modes_tests.rs"]
mod tests;
