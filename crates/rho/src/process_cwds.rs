//! The working directories of the processes running now, so worktree
//! cleanup keeps one a shell, build, or `rho run --worktree` still works in
//! though no daemon session knows of it.

use std::path::PathBuf;

/// The working directory of every process this user may inspect, or
/// `None` where rho cannot read them: on Windows, Unixes other than Linux,
/// Android, and macOS, or when listing processes fails. Processes that
/// exit, or deny access, while it reads are left out. It reads every
/// process, so it runs off the async runtime's workers.
pub(crate) async fn scan() -> Option<Vec<PathBuf>> {
    tokio::task::spawn_blocking(platform::scan)
        .await
        .ok()
        .flatten()
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod platform {
    use std::path::PathBuf;

    pub(super) fn scan() -> Option<Vec<PathBuf>> {
        let entries = std::fs::read_dir("/proc").ok()?;
        Some(
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
                })
                .filter_map(|entry| std::fs::read_link(entry.path().join("cwd")).ok())
                .collect(),
        )
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        ffi::{c_int, c_void, CStr, OsStr},
        os::unix::ffi::OsStrExt,
        path::PathBuf,
    };

    pub(super) fn scan() -> Option<Vec<PathBuf>> {
        // SAFETY: a null buffer asks only for the size the list needs.
        let needed = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return None;
        }
        // Room for processes started between the two calls.
        let mut pids: Vec<c_int> = vec![0; needed as usize + 64];
        let capacity = c_int::try_from(pids.len() * size_of::<c_int>()).ok()?;
        // SAFETY: `pids` holds `capacity` bytes, and the call writes at most
        // that many, returning how many pids it wrote.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), capacity) };
        // A failed call may answer 0 rather than -1; no process running
        // would leave out this one, so 0 is never a real list.
        if count <= 0 {
            return None;
        }
        let count = usize::try_from(count).ok()?.min(pids.len());
        Some(pids[..count].iter().filter_map(|&pid| cwd(pid)).collect())
    }

    fn cwd(pid: c_int) -> Option<PathBuf> {
        // SAFETY: `proc_vnodepathinfo` is plain data, valid zeroed.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = c_int::try_from(size_of::<libc::proc_vnodepathinfo>()).ok()?;
        // SAFETY: `info` is `size` bytes, which the call fills at most.
        let wrote = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                std::ptr::from_mut(&mut info).cast::<c_void>(),
                size,
            )
        };
        if wrote != size {
            return None;
        }
        let path = &info.pvi_cdir.vip_path;
        // SAFETY: `vip_path` is `MAXPATHLEN` contiguous `c_char`s, which
        // the kernel NUL-terminates.
        let bytes =
            unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), size_of_val(path)) };
        let path = CStr::from_bytes_until_nul(bytes).ok()?.to_bytes();
        (!path.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(path)))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
mod platform {
    use std::path::PathBuf;

    pub(super) fn scan() -> Option<Vec<PathBuf>> {
        None
    }
}

#[cfg(test)]
#[path = "process_cwds_tests.rs"]
mod tests;
