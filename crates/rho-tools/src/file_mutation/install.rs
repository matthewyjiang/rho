//! Platform install helpers for atomic no-replace file creation.

use std::path::Path;

use super::{AtomicCreateFaultInjector, AtomicInstallMethod};

pub(super) enum InstallOutcome {
    Installed,
    NotInstalled(std::io::Error),
    #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
    InstalledWithResidual {
        cleanup_error: std::io::Error,
    },
}

pub(super) fn install_no_replace(
    staged: &Path,
    target: &Path,
    method: AtomicInstallMethod,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    match method {
        AtomicInstallMethod::Platform => install_platform_no_replace(staged, target, fault),
        #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
        AtomicInstallMethod::HardLink => install_with_hard_link(staged, target, fault),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    fn c_path(path: &Path) -> std::io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }

    let staged_c = match c_path(staged) {
        Ok(path) => path,
        Err(error) => return InstallOutcome::NotInstalled(error),
    };
    let target_c = match c_path(target) {
        Ok(path) => path,
        Err(error) => return InstallOutcome::NotInstalled(error),
    };
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            staged_c.as_ptr(),
            libc::AT_FDCWD,
            target_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return InstallOutcome::Installed;
    }
    let error = std::io::Error::last_os_error();
    // ENOTSUP may alias EOPNOTSUPP; keep both names for portability.
    #[allow(unreachable_patterns)]
    let unsupported = matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) | Some(libc::ENOTSUP)
    );
    if !unsupported {
        return InstallOutcome::NotInstalled(error);
    }
    install_no_replace(staged, target, AtomicInstallMethod::HardLink, fault)
}

#[cfg(target_vendor = "apple")]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    _fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let staged = match CString::new(staged.as_os_str().as_bytes()) {
        Ok(staged) => staged,
        Err(_) => {
            return InstallOutcome::NotInstalled(std::io::Error::from(
                std::io::ErrorKind::InvalidInput,
            ));
        }
    };
    let target = match CString::new(target.as_os_str().as_bytes()) {
        Ok(target) => target,
        Err(_) => {
            return InstallOutcome::NotInstalled(std::io::Error::from(
                std::io::ErrorKind::InvalidInput,
            ));
        }
    };
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe { libc::renamex_np(staged.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        InstallOutcome::Installed
    } else {
        InstallOutcome::NotInstalled(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    _fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let staged: Vec<u16> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe { MoveFileExW(staged.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if result == 0 {
        InstallOutcome::NotInstalled(std::io::Error::last_os_error())
    } else {
        InstallOutcome::Installed
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    install_no_replace(staged, target, AtomicInstallMethod::HardLink, fault)
}

#[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
fn install_with_hard_link(
    staged: &Path,
    target: &Path,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    if let Err(error) = std::fs::hard_link(staged, target) {
        return InstallOutcome::NotInstalled(error);
    }
    let removal = fault
        .and_then(|fault| fault.fail_staged_removal_after_hard_link(staged))
        .map_or_else(|| std::fs::remove_file(staged), Err);
    match removal {
        Ok(()) => InstallOutcome::Installed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => InstallOutcome::Installed,
        Err(cleanup_error) => InstallOutcome::InstalledWithResidual { cleanup_error },
    }
}
