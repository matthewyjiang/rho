//! A lock for state that belongs to the OS account, not to a rho home: a
//! file in the profile directory of the user this process runs as, which
//! that profile's access control list keeps other users out of.

use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    ptr,
};

use anyhow::Context;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::TOKEN_QUERY,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
    UI::Shell::GetUserProfileDirectoryW,
};

/// Where the lock file `file_name` of the OS account this process runs as
/// is: in the account's own home, which no environment variable redirects.
/// `what` names what it guards, in errors.
pub(crate) fn account_lock_path(file_name: &str, what: &str) -> anyhow::Result<PathBuf> {
    let home = account_home()
        .with_context(|| format!("could not look up the account profile for the {what} lock"))?
        .canonicalize()
        .context("could not resolve the account profile")?;
    if !home.is_dir() {
        anyhow::bail!("{what} lock home {} must be a directory", home.display());
    }
    Ok(home.join(file_name))
}

/// The profile directory of the account, as its token names it: `USERPROFILE`
/// and `RHO_HOME` cannot redirect it.
fn account_home() -> io::Result<PathBuf> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no
    // closing, and `token` is a valid place for the handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut len = 0;
    // SAFETY: a null buffer of length 0 is how the needed length is asked
    // for; the call fails and writes only `len`.
    unsafe { GetUserProfileDirectoryW(token, ptr::null_mut(), &mut len) };
    let mut home = vec![0_u16; len as usize];
    // SAFETY: the buffer is writable for the `len` units the call asked for.
    let read = unsafe { GetUserProfileDirectoryW(token, home.as_mut_ptr(), &mut len) };
    let error = io::Error::last_os_error();
    // SAFETY: `token` is the open handle `OpenProcessToken` returned.
    unsafe { CloseHandle(token) };
    if read == 0 {
        return Err(error);
    }
    let end = home
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(home.len());
    Ok(OsString::from_wide(&home[..end]).into())
}

/// Opens the account lock file `path`, creating it private, and takes its
/// exclusive lock, waiting for another holder.
pub(crate) fn lock_account_file(path: &Path, what: &str) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("could not open {what} mutation lock {}", path.display()))?;
    fs2::FileExt::lock_exclusive(&file)
        .with_context(|| format!("could not lock {what} mutations at {}", path.display()))?;
    Ok(file)
}
