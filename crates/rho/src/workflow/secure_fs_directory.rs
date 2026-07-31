#[cfg(unix)]
use super::secure_fs::{open_beneath_from_file, verified_from_open_file};
use super::{
    secure_fs::{identity_drift, inspect_absolute, SecureDirectory, VerifiedPath},
    FrozenPathKind, WorkflowResult,
};
use std::{ffi::OsString, fs::File, io, path::Path};

impl SecureDirectory {
    pub(crate) fn directory_names(&self, relative: &Path) -> WorkflowResult<Vec<OsString>> {
        let directory = self.open_directory(relative)?;
        Ok(names(&directory)?)
    }
}

pub(super) fn opened_names(file: &File) -> io::Result<Vec<OsString>> {
    names(file)
}

pub(crate) fn open_verified_directory(path: &Path) -> WorkflowResult<VerifiedPath> {
    inspect_absolute(path, FrozenPathKind::Directory, false)
}

pub(crate) fn opened_directory_names(directory: &VerifiedPath) -> WorkflowResult<Vec<OsString>> {
    if directory.identity.kind != FrozenPathKind::Directory {
        return Err(identity_drift(
            Path::new(&directory.identity.canonical_path),
            "expected an opened directory",
        ));
    }
    Ok(opened_names(&directory.file)?)
}

#[cfg(unix)]
pub(crate) fn open_verified_file_in_directory(
    directory: &VerifiedPath,
    relative: &Path,
    hash: bool,
) -> WorkflowResult<VerifiedPath> {
    if directory.identity.kind != FrozenPathKind::Directory {
        return Err(identity_drift(
            Path::new(&directory.identity.canonical_path),
            "expected an opened directory",
        ));
    }
    let file = open_beneath_from_file(
        directory.file.try_clone()?,
        relative,
        FrozenPathKind::File,
        false,
    )?;
    verified_from_open_file(
        file,
        Path::new(&directory.identity.canonical_path).join(relative),
        FrozenPathKind::File,
        hash,
    )
}

#[cfg(not(unix))]
pub(crate) fn open_verified_file_in_directory(
    directory: &VerifiedPath,
    relative: &Path,
    _hash: bool,
) -> WorkflowResult<VerifiedPath> {
    Err(identity_drift(
        &Path::new(&directory.identity.canonical_path).join(relative),
        "handle-relative catalog reads are unsupported on this platform",
    ))
}

#[cfg(unix)]
fn names(file: &File) -> io::Result<Vec<OsString>> {
    use std::{ffi::CStr, os::fd::IntoRawFd as _, os::unix::ffi::OsStrExt as _};

    let fd = file.try_clone()?.into_raw_fd();
    // SAFETY: fd is a new owned directory descriptor. fdopendir owns it after success.
    let directory = unsafe { libc::fdopendir(fd) };
    if directory.is_null() {
        // SAFETY: fdopendir failed and did not take ownership of fd.
        unsafe { libc::close(fd) };
        return Err(io::Error::last_os_error());
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: directory remains valid until closedir below.
        let entry = unsafe { libc::readdir(directory) };
        if entry.is_null() {
            break;
        }
        // SAFETY: d_name is NUL-terminated for the entry returned by readdir.
        let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(std::ffi::OsStr::from_bytes(bytes).to_owned());
        }
    }
    // SAFETY: directory was returned by fdopendir and has not been closed.
    if unsafe { libc::closedir(directory) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(names)
}

#[cfg(windows)]
fn names(file: &File) -> io::Result<Vec<OsString>> {
    use std::{os::windows::ffi::OsStringExt as _, os::windows::io::AsRawHandle as _};
    use windows_sys::Win32::{
        Foundation::{ERROR_HANDLE_EOF, ERROR_NO_MORE_FILES},
        Storage::FileSystem::{
            FileIdBothDirectoryInfo, GetFileInformationByHandleEx, FILE_ID_BOTH_DIR_INFO,
        },
    };

    // Receipt: 1024 u64 values hold at least one fixed directory record plus the Windows
    // maximum 255 UTF-16-code-unit component, while preserving record alignment.
    let mut buffer = [0u64; 1024];
    let mut names = Vec::new();
    loop {
        // SAFETY: buffer is writable and aligned; file remains open for the call.
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdBothDirectoryInfo,
                buffer.as_mut_ptr().cast(),
                std::mem::size_of_val(&buffer) as u32,
            )
        };
        if succeeded == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32)
                || error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32)
            {
                break;
            }
            return Err(error);
        }
        let mut offset = 0usize;
        loop {
            // SAFETY: Windows filled the buffer with aligned FILE_ID_BOTH_DIR_INFO records.
            let entry = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            // SAFETY: FileNameLength describes UTF-16 bytes within this returned record.
            let name = unsafe {
                std::slice::from_raw_parts(
                    entry.FileName.as_ptr(),
                    entry.FileNameLength as usize / std::mem::size_of::<u16>(),
                )
            };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                names.push(OsString::from_wide(name));
            }
            if entry.NextEntryOffset == 0 {
                break;
            }
            offset += entry.NextEntryOffset as usize;
        }
    }
    Ok(names)
}

#[cfg(not(any(unix, windows)))]
fn names(_file: &File) -> io::Result<Vec<OsString>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "handle-based directory enumeration is unavailable",
    ))
}
