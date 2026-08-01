//! Windows-only path identity helpers for secure filesystem opens.

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use super::{secure_fs::identity_drift, WorkflowResult};

pub(super) fn validate_opened_windows_path(file: &File, expected: &Path) -> WorkflowResult<()> {
    let opened = opened_windows_path(file)?;
    if !windows_paths_match(&opened, expected) {
        return Err(identity_drift(
            expected,
            "opened Windows handle resolves outside the requested path",
        ));
    }
    Ok(())
}

pub(crate) fn windows_paths_match(left: &Path, right: &Path) -> bool {
    let opened_key = windows_path_compare_key(left);
    let expected_key = windows_path_compare_key(right);
    wide_eq_ignore_ascii_case(&opened_key, &expected_key)
}

pub(super) fn opened_windows_path(file: &File) -> WorkflowResult<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::{ffi::OsStringExt as _, io::AsRawHandle as _},
    };
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: file owns a valid handle and buffer is writable for its full length.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        return Err(identity_drift(
            Path::new("<handle>"),
            "opened Windows path is unavailable",
        ));
    }
    buffer.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

pub(super) fn windows_path_compare_key(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    let mut raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim_prefix = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    if raw.starts_with(&verbatim_prefix) {
        raw.drain(..verbatim_prefix.len());
    }
    let unc_prefix = [b'U' as u16, b'N' as u16, b'C' as u16, b'\\' as u16];
    let mut normalized = if raw.starts_with(&unc_prefix) {
        let mut unc = vec![b'\\' as u16, b'\\' as u16];
        unc.extend_from_slice(&raw[unc_prefix.len()..]);
        unc
    } else {
        raw
    };
    for unit in &mut normalized {
        if *unit == b'/' as u16 {
            *unit = b'\\' as u16;
        }
    }
    while normalized.last() == Some(&(b'\\' as u16)) {
        normalized.pop();
    }
    normalized
}

fn wide_eq_ignore_ascii_case(left: &[u16], right: &[u16]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| ascii_lowercase(*left) == ascii_lowercase(*right))
}

fn ascii_lowercase(unit: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&unit) {
        unit + (b'a' - b'A') as u16
    } else {
        unit
    }
}
