//! Process standard-stream helpers.

use std::{io::IsTerminal, sync::OnceLock};

/// Whether stdin is a terminal, probed once per process.
///
/// Call this instead of `std::io::stdin().is_terminal()`. On Windows that
/// probe asks a pipe for its name, and the query waits behind any pending
/// synchronous read on the same handle. `rho acp` keeps such a read open on
/// stdin for its whole life, so probing during a session hung `session/new`.
/// Startup calls this before anything reads stdin, and stdin's kind never
/// changes afterwards.
pub(crate) fn stdin_is_terminal() -> bool {
    static STDIN_IS_TERMINAL: OnceLock<bool> = OnceLock::new();
    *STDIN_IS_TERMINAL.get_or_init(|| std::io::stdin().is_terminal())
}

/// True when stdin is a pipe, socket, or redirected file rather than a TTY or
/// null device.
///
/// Automation uses this so `rho run` can reject redirected stdin unless
/// `--stdin` is set, without forcing the flag for terminal or `/dev/null` input.
pub(crate) fn stdin_is_redirected() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        // Inspect stdin's open file description directly. `/dev/fd/0` metadata
        // is unreliable on macOS CI, which made piped `rho run` input look like
        // a terminal/null device and skip the `--stdin` guard.
        let fd = std::io::stdin().as_raw_fd();
        // SAFETY: `libc::stat` is a plain POD out-parameter, and `fstat` only
        // reads the borrowed stdin descriptor for the duration of the call.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return false;
        }
        let mode = stat.st_mode & libc::S_IFMT;
        mode == libc::S_IFIFO || mode == libc::S_IFREG || mode == libc::S_IFSOCK
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Storage::FileSystem::{GetFileType, FILE_TYPE_DISK, FILE_TYPE_PIPE, FILE_TYPE_REMOTE},
            System::Console::{GetStdHandle, STD_INPUT_HANDLE},
        };

        // SAFETY: GetStdHandle/GetFileType read the process standard handle type
        // without retaining the handle beyond this call.
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            matches!(
                GetFileType(handle),
                FILE_TYPE_PIPE | FILE_TYPE_DISK | FILE_TYPE_REMOTE
            )
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}
