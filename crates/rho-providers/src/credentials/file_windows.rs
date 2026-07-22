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
    Storage::FileSystem::FILE_ALL_ACCESS,
    System::WindowsProgramming::GetUserNameW,
};

use super::{CredentialError, CredentialResult};

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
