//! Process standard-stream helpers.

/// True when stdin is a pipe, socket, or redirected file rather than a TTY or
/// null device.
///
/// Automation uses this so `rho run` can reject redirected stdin unless
/// `--stdin` is set, without forcing the flag for terminal or `/dev/null` input.
pub(crate) fn stdin_is_redirected() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        // `/dev/fd/0` reflects the open stdin file description (pipe, file,
        // socket, or char device such as a TTY or null).
        let Ok(metadata) = std::fs::metadata("/dev/fd/0") else {
            return false;
        };
        let file_type = metadata.file_type();
        file_type.is_fifo() || file_type.is_file() || file_type.is_socket()
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
