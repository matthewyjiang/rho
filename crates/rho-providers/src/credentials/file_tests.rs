use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

#[test]
fn file_store_round_trips_secrets_under_rho_home() {
    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();

    assert_eq!(store.get_secret("acct").unwrap(), None);
    store.set_secret("acct", "secret-value").unwrap();
    assert_eq!(
        store.get_secret("acct").unwrap().as_deref(),
        Some("secret-value")
    );
    assert!(store.delete_secret("acct").unwrap());
    assert_eq!(store.get_secret("acct").unwrap(), None);
    assert!(!store.delete_secret("acct").unwrap());
}

#[test]
fn file_store_preserves_unrelated_secrets_across_updates() {
    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_directory(root.path().join("credentials")).unwrap();

    store.set_secret("one", "a").unwrap();
    store.set_secret("two", "b").unwrap();
    store.set_secret("one", "a2").unwrap();

    assert_eq!(store.get_secret("one").unwrap().as_deref(), Some("a2"));
    assert_eq!(store.get_secret("two").unwrap().as_deref(), Some("b"));
}

#[cfg(unix)]
#[test]
fn file_store_uses_private_unix_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();
    store.set_secret("acct", "secret").unwrap();

    let dir_mode = fs::metadata(store.directory())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let file_mode = fs::metadata(store.secrets_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let lock_mode = fs::metadata(store.directory().join("secrets.lock"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
    assert_eq!(lock_mode, 0o600);
}

#[cfg(windows)]
#[test]
fn file_store_uses_protected_single_user_windows_acl() {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{LocalFree, ERROR_SUCCESS},
        Security::{
            Authorization::{
                GetExplicitEntriesFromAclW, GetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
                SE_FILE_OBJECT,
            },
            GetSecurityDescriptorControl, DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
    };

    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();
    store.set_secret("acct", "secret").unwrap();
    let wide_path: Vec<u16> = store
        .secrets_path()
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut acl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let result = unsafe {
        GetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(result, ERROR_SUCCESS);
    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0
    );
    assert_ne!(control & SE_DACL_PROTECTED, 0);
    let mut count = 0;
    let mut entries: *mut EXPLICIT_ACCESS_W = ptr::null_mut();
    let result = unsafe { GetExplicitEntriesFromAclW(acl, &mut count, &mut entries) };
    assert_eq!(result, ERROR_SUCCESS);
    assert_eq!(count, 1);
    assert_eq!(unsafe { (*entries).grfAccessPermissions }, FILE_ALL_ACCESS);
    unsafe {
        LocalFree(entries.cast());
        LocalFree(descriptor);
    }
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_credential_paths() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let linked_directory = root.path().join("linked-credentials");
    symlink(&outside, &linked_directory).unwrap();
    assert!(FileCredentialStore::with_directory(&linked_directory).is_err());

    let credential_directory = root.path().join("credentials");
    fs::create_dir(&credential_directory).unwrap();
    let outside_file = root.path().join("outside-file");
    fs::write(&outside_file, "keep").unwrap();
    symlink(&outside_file, credential_directory.join(LOCK_FILE_NAME)).unwrap();
    assert!(FileCredentialStore::with_directory(&credential_directory).is_err());
    assert_eq!(fs::read_to_string(outside_file).unwrap(), "keep");
}

#[test]
fn concurrent_writers_do_not_lose_secrets() {
    let root = TempDir::new().unwrap();
    let rho_home = root.path().to_path_buf();
    // Separate store handles share the on-disk lock, not one process mutex.
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();

    for index in 0..8 {
        let rho_home = rho_home.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let store = FileCredentialStore::with_rho_home(&rho_home).unwrap();
            barrier.wait();
            let account = format!("acct-{index}");
            store
                .set_secret(&account, &format!("secret-{index}"))
                .unwrap();
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let store = FileCredentialStore::with_rho_home(&rho_home).unwrap();
    for index in 0..8 {
        assert_eq!(
            store
                .get_secret(&format!("acct-{index}"))
                .unwrap()
                .as_deref(),
            Some(format!("secret-{index}")).as_deref()
        );
    }
}

#[test]
fn removes_stale_secret_temp_files_while_locked() {
    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();
    let stale = store
        .directory()
        .join(format!("{SECRETS_FILE_NAME}.tmp.interrupted"));
    fs::write(&stale, "partial secret").unwrap();

    assert_eq!(store.get_secret("missing").unwrap(), None);
    assert!(!stale.exists());
}

#[test]
fn rejects_empty_account_names() {
    let root = TempDir::new().unwrap();
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();
    assert!(store.set_secret("", "x").is_err());
    assert!(store.get_secret("").is_err());
}

#[test]
fn open_creates_credentials_directory_lazily_under_rho_home() {
    let root = TempDir::new().unwrap();
    let credentials_dir = root.path().join("credentials");
    assert!(!credentials_dir.exists());
    let store = FileCredentialStore::with_rho_home(root.path()).unwrap();
    assert!(store.directory().exists());
    assert_eq!(store.directory(), credentials_dir);
}
