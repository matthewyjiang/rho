use std::path::Path;

use super::{
    secure_fs::{identity_drift, SecureDirectory},
    WorkflowError, WorkflowResult,
};

impl SecureDirectory {
    pub(crate) fn write_file_if_absent(
        &self,
        relative: &Path,
        bytes: &[u8],
    ) -> WorkflowResult<bool> {
        write_file_if_absent_in(self, relative, bytes)
    }

    pub(crate) fn remove_file(&self, relative: &Path) -> WorkflowResult<()> {
        remove_file_in(self, relative)
    }
}

#[cfg(unix)]
fn write_file_if_absent_in(
    root: &SecureDirectory,
    relative: &Path,
    bytes: &[u8],
) -> WorkflowResult<bool> {
    use std::{
        ffi::CString,
        fs::File,
        io::Write as _,
        os::{fd::FromRawFd as _, unix::ffi::OsStrExt as _},
        path::Component,
    };

    let parent = relative
        .parent()
        .ok_or_else(|| identity_drift(relative, "file has no parent"))?;
    let directory = root.open_directory(parent)?;
    let Some(Component::Normal(file_name)) = relative.components().next_back() else {
        return Err(identity_drift(relative, "invalid file name"));
    };
    let file_name = CString::new(file_name.as_bytes())
        .map_err(|_| identity_drift(relative, "NUL in file name"))?;
    let temporary_name = CString::new(format!(".rho-tmp-{}", uuid::Uuid::new_v4()))
        .expect("UUID temporary name has no NUL");
    let directory_fd = std::os::fd::AsRawFd::as_raw_fd(&directory);
    // SAFETY: the held directory descriptor and temporary name are valid.
    let fd = unsafe {
        libc::openat(
            directory_fd,
            temporary_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: openat returned a new owned descriptor.
    let mut temporary = unsafe { File::from_raw_fd(fd) };
    let result = (|| -> std::io::Result<bool> {
        temporary.write_all(bytes)?;
        temporary.sync_all()?;
        // SAFETY: both names stay in the held parent directory.
        let installed = unsafe {
            libc::linkat(
                directory_fd,
                temporary_name.as_ptr(),
                directory_fd,
                file_name.as_ptr(),
                0,
            )
        };
        if installed == 0 {
            directory.sync_all()?;
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Ok(false)
        } else {
            Err(error)
        }
    })();
    // SAFETY: the temporary name and held parent descriptor are valid. The
    // installed hard link keeps the completed bytes alive.
    unsafe { libc::unlinkat(directory_fd, temporary_name.as_ptr(), 0) };
    result.map_err(WorkflowError::Io)
}

#[cfg(unix)]
fn remove_file_in(root: &SecureDirectory, relative: &Path) -> WorkflowResult<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt as _, path::Component};

    let parent = relative
        .parent()
        .ok_or_else(|| identity_drift(relative, "file has no parent"))?;
    let directory = root.open_directory(parent)?;
    let Some(Component::Normal(file_name)) = relative.components().next_back() else {
        return Err(identity_drift(relative, "invalid file name"));
    };
    let file_name = CString::new(file_name.as_bytes())
        .map_err(|_| identity_drift(relative, "NUL in file name"))?;
    // SAFETY: the held directory descriptor and file name are valid.
    if unsafe {
        libc::unlinkat(
            std::os::fd::AsRawFd::as_raw_fd(&directory),
            file_name.as_ptr(),
            0,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error().into());
    }
    directory.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn write_file_if_absent_in(
    root: &SecureDirectory,
    relative: &Path,
    bytes: &[u8],
) -> WorkflowResult<bool> {
    use std::{io::Write as _, os::windows::fs::OpenOptionsExt as _};
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let parent = relative
        .parent()
        .ok_or_else(|| identity_drift(relative, "file has no parent"))?;
    let parent_handle = root.open_directory(parent)?;
    let temporary = parent.join(format!(".rho-tmp-{}", uuid::Uuid::new_v4()));
    let temporary_path = root.path.join(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = options.open(&temporary_path)?;
    super::secure_fs::validate_opened_windows_path(&file, &root.expected_path.join(&temporary))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let installed = match std::fs::hard_link(&temporary_path, root.path.join(relative)) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            return Err(error.into());
        }
    };
    std::fs::remove_file(temporary_path)?;
    super::secure_fs::validate_opened_windows_path(
        &parent_handle,
        &root.expected_path.join(parent),
    )?;
    Ok(installed)
}

#[cfg(windows)]
fn remove_file_in(root: &SecureDirectory, relative: &Path) -> WorkflowResult<()> {
    let parent = relative
        .parent()
        .ok_or_else(|| identity_drift(relative, "file has no parent"))?;
    let parent_handle = root.open_directory(parent)?;
    std::fs::remove_file(root.path.join(relative))?;
    super::secure_fs::validate_opened_windows_path(&parent_handle, &root.expected_path.join(parent))
}

#[cfg(all(not(unix), not(windows)))]
fn write_file_if_absent_in(
    root: &SecureDirectory,
    relative: &Path,
    _bytes: &[u8],
) -> WorkflowResult<bool> {
    Err(identity_drift(
        &root.path.join(relative),
        "handle-relative create-if-absent is unsupported on this platform",
    ))
}

#[cfg(all(not(unix), not(windows)))]
fn remove_file_in(root: &SecureDirectory, relative: &Path) -> WorkflowResult<()> {
    Err(identity_drift(
        &root.path.join(relative),
        "handle-relative removal is unsupported on this platform",
    ))
}
