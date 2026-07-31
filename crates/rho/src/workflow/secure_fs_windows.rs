//! Windows-only path identity helpers for secure filesystem opens.

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use super::{secure_fs::identity_drift, WorkflowResult};

pub(super) fn validate_opened_windows_path(file: &File, expected: &Path) -> WorkflowResult<()> {
    let opened = opened_windows_path(file)?;
    let opened_key = windows_path_compare_key(&opened);
    let expected_key = windows_path_compare_key(expected);
    if !opened_key.eq_ignore_ascii_case(&expected_key) {
        return Err(identity_drift(
            expected,
            "opened Windows handle resolves outside the requested path",
        ));
    }
    Ok(())
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
    let opened = OsString::from_wide(&buffer).to_string_lossy().into_owned();
    Ok(PathBuf::from(windows_path_compare_key(Path::new(&opened))))
}

fn windows_path_compare_key(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw.as_ref());
    let normalized = if let Some(unc) = stripped.strip_prefix(r"UNC\") {
        format!(r"\\{unc}")
    } else {
        stripped.replace('/', "\\")
    };
    normalized.trim_end_matches(['\\', '/']).to_owned()
}
