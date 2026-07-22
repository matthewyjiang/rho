use std::{fs::File, io};

use super::{CredentialError, CredentialResult};

pub(super) struct FileLock {
    file: File,
}

impl FileLock {
    pub(super) fn acquire(file: File) -> CredentialResult<Self> {
        lock_exclusive(&file)?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

fn lock_exclusive(file: &File) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(CredentialError::StoreUnavailable(format!(
                "could not lock credential store: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::{os::windows::io::AsRawHandle, ptr};

        #[link(name = "kernel32")]
        extern "system" {
            fn LockFileEx(
                file: *mut core::ffi::c_void,
                flags: u32,
                reserved: u32,
                bytes_low: u32,
                bytes_high: u32,
                overlapped: *mut Overlapped,
            ) -> i32;
        }

        #[repr(C)]
        struct Overlapped {
            internal: usize,
            internal_high: usize,
            offset: u32,
            offset_high: u32,
            event: *mut core::ffi::c_void,
        }

        const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x2;
        let mut overlapped = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: ptr::null_mut(),
        };
        let result = unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK,
                0,
                1,
                0,
                &mut overlapped,
            )
        };
        if result == 0 {
            return Err(CredentialError::StoreUnavailable(format!(
                "could not lock credential store: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(())
    }
}

fn unlock_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::{os::windows::io::AsRawHandle, ptr};

        #[link(name = "kernel32")]
        extern "system" {
            fn UnlockFileEx(
                file: *mut core::ffi::c_void,
                reserved: u32,
                bytes_low: u32,
                bytes_high: u32,
                overlapped: *mut Overlapped,
            ) -> i32;
        }

        #[repr(C)]
        struct Overlapped {
            internal: usize,
            internal_high: usize,
            offset: u32,
            offset_high: u32,
            event: *mut core::ffi::c_void,
        }

        let mut overlapped = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: ptr::null_mut(),
        };
        let result = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(())
    }
}
