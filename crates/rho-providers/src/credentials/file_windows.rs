use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr};

use windows_sys::Win32::{
    Foundation::{LocalFree, ERROR_SUCCESS},
    Security::{
        Authorization::{
            BuildExplicitAccessWithNameW, SetEntriesInAclW, SetNamedSecurityInfoW,
            EXPLICIT_ACCESS_W, SET_ACCESS, SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION, NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    },
    Storage::FileSystem::{
        MoveFileExW, FILE_ALL_ACCESS, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    },
    System::WindowsProgramming::GetUserNameW,
};

use super::{CredentialError, CredentialResult};

/// Atomically replace `destination` with `source`, overwriting when needed.
pub(super) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn set_private_windows_acl(path: &Path, directory: bool) -> CredentialResult<()> {
    let mut name_len = 0;
    unsafe { GetUserNameW(ptr::null_mut(), &mut name_len) };
    if name_len == 0 {
        return Err(CredentialError::StoreUnavailable(format!(
            "could not resolve current Windows user: {}",
            io::Error::last_os_error()
        )));
    }
    let mut username = vec![0u16; name_len as usize];
    if unsafe { GetUserNameW(username.as_mut_ptr(), &mut name_len) } == 0 {
        return Err(CredentialError::StoreUnavailable(format!(
            "could not resolve current Windows user: {}",
            io::Error::last_os_error()
        )));
    }

    let mut access = EXPLICIT_ACCESS_W::default();
    unsafe {
        BuildExplicitAccessWithNameW(
            &mut access,
            username.as_mut_ptr(),
            FILE_ALL_ACCESS,
            SET_ACCESS,
            if directory {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                NO_INHERITANCE
            },
        );
    }
    let mut acl = ptr::null_mut();
    let result = unsafe { SetEntriesInAclW(1, &access, ptr::null(), &mut acl) };
    if result != ERROR_SUCCESS {
        return Err(CredentialError::StoreUnavailable(format!(
            "could not build private ACL for {}: Windows error {result}",
            path.display()
        )));
    }

    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    unsafe { LocalFree(acl.cast()) };
    if result != ERROR_SUCCESS {
        return Err(CredentialError::StoreUnavailable(format!(
            "could not protect credential path {}: Windows error {result}",
            path.display()
        )));
    }
    Ok(())
}
