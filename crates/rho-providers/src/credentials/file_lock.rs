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
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK},
            System::IO::OVERLAPPED,
        };

        let mut overlapped = OVERLAPPED::default();
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
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};

        let mut overlapped = OVERLAPPED::default();
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
