//! Cross-platform exclusive advisory file locking.
//!
//! Generic infrastructure shared by everything that compare-and-replaces a
//! file across processes (the file credential store, Claude rate-limit state).
//! Callers pick a wait policy explicitly: [`FileLock::acquire`] blocks until the
//! lock is free, [`FileLock::acquire_with_retry`] gives up after a bounded wait
//! so a wedged holder cannot stall a caller forever.

use std::{fs::File, io, thread, time::Duration};

/// Exclusive advisory lock, released when the value is dropped.
///
/// The lock owns its file handle: dropping the guard unlocks and closes it.
pub struct FileLock {
    file: File,
}

impl FileLock {
    /// Block until the exclusive lock is held.
    pub fn acquire(file: File) -> io::Result<Self> {
        lock_exclusive_blocking(&file)?;
        Ok(Self { file })
    }

    /// Take the lock, retrying while another holder has it.
    ///
    /// Sleeps `delay` between attempts and returns the busy error after
    /// `attempts` retries. Interruptions are retried without counting against
    /// the delay budget's sleep.
    pub fn acquire_with_retry(file: File, attempts: u32, delay: Duration) -> io::Result<Self> {
        let mut remaining = attempts;
        loop {
            match try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { file }),
                // Interruptions are transient: retry immediately without
                // consuming the busy-retry budget or sleeping.
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if is_lock_busy(&error) => {
                    if remaining == 0 {
                        return Err(error);
                    }
                    remaining -= 1;
                    thread::sleep(delay);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

fn lock_exclusive_blocking(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
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

        // OVERLAPPED must be zeroed; Default is not guaranteed to zero reserved fields.
        let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };
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

fn try_lock_exclusive(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::GetLastError,
            Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY},
            System::IO::OVERLAPPED,
        };

        let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };
        let result = unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut overlapped,
            )
        };
        if result != 0 {
            return Ok(());
        }
        let code = unsafe { GetLastError() };
        Err(io::Error::from_raw_os_error(code as i32))
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
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};

        let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };
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

/// True when the error means another holder currently owns the lock.
fn is_lock_busy(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    // Windows ERROR_LOCK_VIOLATION / ERROR_BUSY often map as Other, and
    // EWOULDBLOCK and EAGAIN are identical on many unix targets.
    #[cfg(windows)]
    {
        const ERROR_LOCK_VIOLATION: i32 = 33;
        const ERROR_BUSY: i32 = 170;
        matches!(
            error.raw_os_error(),
            Some(ERROR_LOCK_VIOLATION | ERROR_BUSY)
        )
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}
