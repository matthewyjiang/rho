use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::windows::io::AsRawHandle,
};
use windows_sys::Win32::System::Console::{
    GetConsoleMode, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_INPUT,
};

pub(super) fn console() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("CONIN$")
}

pub(super) fn mode(console: &File) -> io::Result<u32> {
    let mut mode = 0;
    if unsafe { GetConsoleMode(console.as_raw_handle(), &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

/// Own only the VT input bit, leaving raw mode and mouse flags to their guards.
/// Unsupported consoles retain the released crossterm Win32 reader.
pub(in crate::tui) struct PasteMode {
    console: File,
    original_vt: u32,
    enabled: bool,
}

impl PasteMode {
    pub(in crate::tui) fn acquire() -> io::Result<Self> {
        let console = console()?;
        let original = mode(&console)?;
        let mut guard = Self {
            console,
            original_vt: original & ENABLE_VIRTUAL_TERMINAL_INPUT,
            enabled: false,
        };
        // Flush older terminal commands before switching the input transport.
        io::stdout().flush()?;
        if unsafe {
            SetConsoleMode(
                guard.console.as_raw_handle(),
                original | ENABLE_VIRTUAL_TERMINAL_INPUT,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        guard.enabled = true;
        let mut stdout = io::stdout();
        stdout.write_all(b"\x1b[?2004h")?;
        stdout.flush()?;
        Ok(guard)
    }

    pub(in crate::tui) fn release(mut self) -> io::Result<()> {
        self.restore()
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let mut stdout = io::stdout();
        // Normally disable the protocol before restoring its transport. Even
        // if stdout has failed, cleanup must still restore the console bit.
        let output_result = stdout
            .write_all(b"\x1b[?2004l")
            .and_then(|()| stdout.flush());
        let current = mode(&self.console)?;
        let restored = (current & !ENABLE_VIRTUAL_TERMINAL_INPUT) | self.original_vt;
        if unsafe { SetConsoleMode(self.console.as_raw_handle(), restored) } == 0 {
            return Err(io::Error::last_os_error());
        }
        self.enabled = false;
        output_result
    }
}

impl Drop for PasteMode {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
