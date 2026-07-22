//! Private path validation and permission helpers for the file credential store.

use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

#[cfg(windows)]
use super::file_windows::set_private_windows_acl;
use super::{CredentialError, CredentialResult};

/// How to open a private credential-store path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrivateFileOpen {
    /// Open an existing file for read/write.
    Existing,
    /// Open an existing file or create it if missing; do not truncate.
    OpenOrCreate,
    /// Create a new file exclusively (fails if the path already exists).
    CreateNew,
}

pub(super) fn open_private_file(path: &Path, mode: PrivateFileOpen) -> CredentialResult<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    match mode {
        PrivateFileOpen::Existing => {}
        PrivateFileOpen::OpenOrCreate => {
            options.create(true);
        }
        PrivateFileOpen::CreateNew => {
            options.create_new(true);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|err| {
        CredentialError::StoreUnavailable(format!(
            "could not open credential path {}: {err}",
            path.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|err| {
                CredentialError::StoreUnavailable(format!(
                    "could not set permissions on {}: {err}",
                    path.display()
                ))
            })?;
    }
    validate_private_file(&file, path)?;
    Ok(file)
}

pub(super) fn ensure_private_directory(path: &Path) -> CredentialResult<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|err| {
        CredentialError::StoreUnavailable(format!(
            "could not create credential directory {}: {err}",
            path.display()
        ))
    })?;
    validate_private_directory(path)?;
    set_private_directory_permissions(path)?;
    Ok(())
}

pub(super) fn validate_private_directory(path: &Path) -> CredentialResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CredentialError::StoreUnavailable(format!(
            "could not inspect credential directory {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CredentialError::StoreUnavailable(format!(
            "credential directory {} is not a real directory",
            path.display()
        )));
    }
    validate_owner(&metadata, path)
}

pub(super) fn validate_private_file(file: &File, path: &Path) -> CredentialResult<()> {
    let metadata = file.metadata().map_err(|error| {
        CredentialError::StoreUnavailable(format!(
            "could not inspect credential path {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(CredentialError::StoreUnavailable(format!(
            "credential path {} is not a regular file",
            path.display()
        )));
    }
    validate_owner(&metadata, path)
}

pub(super) fn validate_owner(metadata: &fs::Metadata, path: &Path) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CredentialError::StoreUnavailable(format!(
                "credential path {} is not owned by the current user",
                path.display()
            )));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(CredentialError::StoreUnavailable(format!(
                "credential path {} is a Windows reparse point",
                path.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn set_private_directory_permissions(path: &Path) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
            CredentialError::StoreUnavailable(format!(
                "could not set permissions on {}: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(windows)]
    set_private_windows_acl(path, /*directory*/ true)?;
    Ok(())
}

pub(super) fn set_private_file_permissions(path: &Path) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            CredentialError::StoreUnavailable(format!(
                "could not set permissions on {}: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(windows)]
    set_private_windows_acl(path, /*directory*/ false)?;
    Ok(())
}

pub(super) fn validate_account(account: &str) -> CredentialResult<()> {
    if account.is_empty() {
        return Err(CredentialError::InvalidData(
            "credential account name cannot be empty".into(),
        ));
    }
    if account.contains('\0') {
        return Err(CredentialError::InvalidData(
            "credential account name cannot contain NUL".into(),
        ));
    }
    Ok(())
}
