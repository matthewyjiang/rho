use std::{fs, io::Write, path::PathBuf};

pub(super) fn write_atomically(path: &PathBuf, contents: &str) -> anyhow::Result<()> {
    let temp_path = path.with_extension(format!("toml.{}.tmp", uuid::Uuid::new_v4()));
    let mut temp_file = fs::File::create(&temp_path)?;
    set_private_file_permissions(&temp_file)?;
    temp_file.write_all(contents.as_bytes())?;
    temp_file.sync_all()?;
    drop(temp_file);
    replace_file(&temp_path, path)?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(temp_path: &PathBuf, path: &PathBuf) -> anyhow::Result<()> {
    fs::rename(temp_path, path)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(temp_path: &PathBuf, path: &PathBuf) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    if !path.exists() {
        fs::rename(temp_path, path)?;
        return Ok(());
    }
    let replaced = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = temp_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &fs::File) -> anyhow::Result<()> {
    Ok(())
}
