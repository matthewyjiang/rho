//! Keyboard and paste input for a prompt printed on stderr outside the
//! full-screen TUI, such as API-key login.

use std::io;

use crossterm::{
    event::Event,
    terminal::{disable_raw_mode, enable_raw_mode},
};

use super::{paste_mode::PasteMode, terminal_events::TerminalEvents};

/// Raw-mode terminal input with bracketed paste, read through the same event
/// path as the TUI so Windows consoles decode paste too.
///
/// Fields drop in teardown order: the event reader stops before paste mode
/// restores the console, and raw mode ends last.
pub(crate) struct InlinePromptInput {
    events: TerminalEvents,
    // Terminals without bracketed paste still deliver typed keys.
    _paste: Option<PasteMode<io::Stderr>>,
    _raw: RawMode,
}

impl InlinePromptInput {
    pub(crate) fn acquire() -> io::Result<Self> {
        enable_raw_mode()?;
        let raw = RawMode;
        let paste = PasteMode::acquire(io::stderr()).ok();
        Ok(Self {
            // The reader starts on the first `next`, after paste mode is set.
            events: TerminalEvents::new(),
            _paste: paste,
            _raw: raw,
        })
    }

    pub(crate) async fn next(&mut self) -> io::Result<Event> {
        self.events.next().await
    }
}

struct RawMode;

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}
