use std::{
    fs::{File, OpenOptions},
    io,
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

/// Own only the console VT input bit, leaving raw mode and mouse flags to their
/// guards. While it is set, `TerminalEvents` decodes paste at the console-record
/// boundary; unsupported consoles retain the released crossterm Win32 reader.
pub(in crate::tui) struct VtInput {
    console: File,
    original_vt: u32,
    enabled: bool,
}

impl VtInput {
    pub(in crate::tui) fn acquire() -> io::Result<Self> {
        let console = console()?;
        let original = mode(&console)?;
        if unsafe {
            SetConsoleMode(
                console.as_raw_handle(),
                original | ENABLE_VIRTUAL_TERMINAL_INPUT,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            console,
            original_vt: original & ENABLE_VIRTUAL_TERMINAL_INPUT,
            enabled: true,
        })
    }

    pub(in crate::tui) fn restore(&mut self) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let current = mode(&self.console)?;
        let restored = (current & !ENABLE_VIRTUAL_TERMINAL_INPUT) | self.original_vt;
        if unsafe { SetConsoleMode(self.console.as_raw_handle(), restored) } == 0 {
            return Err(io::Error::last_os_error());
        }
        self.enabled = false;
        Ok(())
    }
}

impl Drop for VtInput {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
