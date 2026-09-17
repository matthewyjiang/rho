//! Open catalog children relative to the authorized directory handle, not its reusable path.

use std::{
    fs::File,
    os::windows::{
        ffi::OsStrExt as _,
        fs::MetadataExt as _,
        io::{AsRawHandle as _, FromRawHandle as _},
    },
    path::{Component, Path},
    ptr,
};

use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtCreateFile, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::{RtlNtStatusToDosError, UNICODE_STRING},
        Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

use crate::workflow::{secure_fs::identity_drift, WorkflowError, WorkflowResult};

pub(super) fn open_file_beneath(mut current: File, relative: &Path) -> WorkflowResult<File> {
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Err(identity_drift(relative, "expected a file"));
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(identity_drift(relative, "path traversal is not allowed"));
        };
        let mut name: Vec<u16> = name.encode_wide().collect();
        // NT names are counted, but NUL and ':' must not select truncated names or streams.
        if name.iter().any(|unit| *unit == 0 || *unit == b':' as u16) {
            return Err(identity_drift(
                relative,
                "NUL or alternate data stream in file name",
            ));
        }
        let byte_len = std::mem::size_of_val(name.as_slice());
        let byte_len = u16::try_from(byte_len).map_err(|_| {
            identity_drift(
                relative,
                &format!(
                    "Windows UNICODE_STRING name bytes: limit {}, asked {byte_len}",
                    u16::MAX
                ),
            )
        })?;
        let mut name = UNICODE_STRING {
            Length: byte_len,
            MaximumLength: byte_len,
            Buffer: name.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: current.as_raw_handle(),
            ObjectName: &mut name,
            // Preserve the exact enumerated name, including in case-sensitive directories.
            Attributes: 0,
            SecurityDescriptor: ptr::null_mut(),
            SecurityQualityOfService: ptr::null_mut(),
        };
        let mut status_block = IO_STATUS_BLOCK::default();
        let mut handle = ptr::null_mut();
        let last = index + 1 == components.len();
        let kind = if last {
            FILE_NON_DIRECTORY_FILE
        } else {
            FILE_DIRECTORY_FILE
        };
        // SAFETY: the parent handle, counted UTF-16 name, and output buffers remain valid
        // throughout this synchronous open. On success the returned handle is owned below.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                FILE_GENERIC_READ,
                &attributes,
                &mut status_block,
                ptr::null(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN,
                kind | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                ptr::null(),
                0,
            )
        };
        if status < 0 {
            // SAFETY: status is the NTSTATUS returned by NtCreateFile.
            let error = unsafe { RtlNtStatusToDosError(status) };
            return Err(std::io::Error::from_raw_os_error(error as i32).into());
        }
        // SAFETY: NtCreateFile succeeded and returned a new owned synchronous handle.
        current = unsafe { File::from_raw_handle(handle) };
        let metadata = current.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(WorkflowError::SourceSymlink {
                path: relative.to_owned(),
            });
        }
        if (last && !metadata.is_file()) || (!last && !metadata.is_dir()) {
            return Err(identity_drift(relative, "path kind changed"));
        }
    }
    Ok(current)
}
