use std::{fs::File, path::Path};

use super::{WorkflowError, WorkflowResult};

/// Whether a target's OS adapter can launch a frozen executable, its script
/// interpreter, and its working directory from the handles Rho verified, so a
/// path replaced after the check cannot select another object.
///
/// Frozen command nodes and external CLI agent nodes need this launch;
/// in-process Rho agent nodes do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandleLaunch {
    Supported,
    Unsupported,
}

impl HandleLaunch {
    /// The support of the target Rho was built for.
    pub(crate) const CURRENT: Self = if cfg!(any(target_os = "linux", target_os = "android")) {
        Self::Supported
    } else {
        Self::Unsupported
    };

    /// Fails with the platform limit when frozen processes cannot launch.
    pub(crate) fn ensure(self) -> WorkflowResult<()> {
        match self {
            Self::Supported => Ok(()),
            Self::Unsupported => Err(WorkflowError::UnsupportedPlatform),
        }
    }
}

pub(crate) fn verified_handle_path(
    file: &File,
    _fallback: &Path,
) -> WorkflowResult<std::path::PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd as _;
        Ok(std::path::PathBuf::from(format!(
            "/proc/self/fd/{}",
            file.as_raw_fd()
        )))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = file;
        Err(WorkflowError::UnsupportedPlatform)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn configure_handle_inheritance(
    command: &mut tokio::process::Command,
    files: &[&File],
) -> WorkflowResult<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::process::CommandExt as _;

    let descriptors = files
        .iter()
        .map(|file| file.as_raw_fd())
        .collect::<Vec<_>>();
    // SAFETY: the closure runs after fork and before exec. It only calls
    // async-signal-safe fcntl operations on descriptors kept alive by `files`'
    // owners. Any failure aborts the spawn before exec.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            for &fd in &descriptors {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) fn configure_handle_inheritance(
    _command: &mut tokio::process::Command,
    _files: &[&File],
) -> WorkflowResult<()> {
    Err(WorkflowError::UnsupportedPlatform)
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
#[path = "secure_fs_spawn_tests.rs"]
mod tests;
