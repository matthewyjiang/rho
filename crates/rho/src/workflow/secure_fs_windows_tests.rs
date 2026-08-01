use std::{ffi::OsString, os::windows::ffi::OsStringExt as _, path::PathBuf};

use super::secure_fs_windows::windows_path_compare_key;

// Covers: distinct non-Unicode Windows paths must not collapse to one identity key.
// Owner: secure filesystem Windows identity adapter.
#[test]
fn path_identity_key_preserves_invalid_utf16() {
    let prefix = [b'C' as u16, b':' as u16, b'\\' as u16];
    let left = PathBuf::from(OsString::from_wide(
        &[prefix.as_slice(), &[0xd800]].concat(),
    ));
    let right = PathBuf::from(OsString::from_wide(
        &[prefix.as_slice(), &[0xd801]].concat(),
    ));

    assert_ne!(
        windows_path_compare_key(&left),
        windows_path_compare_key(&right)
    );
}
