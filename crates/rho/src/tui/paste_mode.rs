//! Bracketed paste for the terminal stream that draws the current UI.

use std::io::{self, Write};

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
};

/// Bracketed paste enabled on `output` until released or dropped.
///
/// On Windows this also owns the console VT input bit: crossterm's legacy
/// Win32 reader cannot decode paste, so `TerminalEvents` reads VT records
/// whenever that bit is set. Acquire this before the first event read.
pub(super) struct PasteMode<W: Write> {
    output: W,
    enabled: bool,
    #[cfg(windows)]
    vt_input: super::terminal_events::VtInput,
}

impl<W: Write> PasteMode<W> {
    pub(super) fn acquire(mut output: W) -> io::Result<Self> {
        // Flush older terminal commands before switching the input transport.
        output.flush()?;
        let mut mode = Self {
            output,
            enabled: false,
            #[cfg(windows)]
            vt_input: super::terminal_events::VtInput::acquire()?,
        };
        // A failed write drops `mode`, which restores the VT input bit.
        execute!(mode.output, EnableBracketedPaste)?;
        mode.enabled = true;
        Ok(mode)
    }

    pub(super) fn release(mut self) -> io::Result<()> {
        self.restore()
    }

    fn restore(&mut self) -> io::Result<()> {
        // Disable the protocol before restoring its transport. Even if the
        // output has failed, cleanup must still restore the console bit.
        let output_result = if std::mem::take(&mut self.enabled) {
            execute!(self.output, DisableBracketedPaste)
        } else {
            Ok(())
        };
        #[cfg(windows)]
        self.vt_input.restore()?;
        output_result
    }
}

impl<W: Write> Drop for PasteMode<W> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
