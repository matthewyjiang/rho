//! Bracketed paste for the terminal stream that draws the current UI.

use std::io::{self, Write};

// Written directly rather than through crossterm's `execute!`, which skips
// ANSI on Windows consoles it deems unsupported even when the host (ConPTY,
// WezTerm) understands paste sequences. `acquire` still runs crossterm's
// output setup so a console with VT output off parses them.
const ENABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004h";
const DISABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004l";

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
        // A failed enable drops `vt_input`, which restores the console bit.
        #[cfg(windows)]
        let vt_input = {
            // Best effort: enables VT output processing; its verdict is not a gate.
            let _ = crossterm::ansi_support::supports_ansi();
            super::terminal_events::VtInput::acquire()?
        };
        write_flushed(&mut output, ENABLE_BRACKETED_PASTE)?;
        Ok(Self {
            output,
            enabled: true,
            #[cfg(windows)]
            vt_input,
        })
    }

    pub(super) fn release(mut self) -> io::Result<()> {
        self.restore()
    }

    fn restore(&mut self) -> io::Result<()> {
        // Disable the protocol before restoring its transport. Even if the
        // output has failed, cleanup must still restore the console bit.
        let output_result = if std::mem::take(&mut self.enabled) {
            write_flushed(&mut self.output, DISABLE_BRACKETED_PASTE)
        } else {
            Ok(())
        };
        #[cfg(windows)]
        self.vt_input.restore()?;
        output_result
    }
}

fn write_flushed(output: &mut impl Write, sequence: &[u8]) -> io::Result<()> {
    output.write_all(sequence)?;
    output.flush()
}

impl<W: Write> Drop for PasteMode<W> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
