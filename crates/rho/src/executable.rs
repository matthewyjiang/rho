use std::{
    env,
    path::{Component, Path, PathBuf},
};

/// Finds a bare executable name on `PATH` without spawning it.
///
/// On Windows, a bare name may omit only the `.exe` extension, matching
/// `std::process::Command`. Shell-only `PATHEXT` entries such as `.cmd` and
/// `.bat` must be named explicitly.
pub(crate) fn find_on_path(program: &str) -> Option<PathBuf> {
    let mut components = Path::new(program).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return None;
    }

    let paths = env::var_os("PATH")?;
    find_in(program, env::split_paths(&paths))
}

fn find_in(program: &str, directories: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    directories
        .into_iter()
        .map(|directory| executable_path(&directory, program))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(windows)]
fn executable_path(directory: &Path, program: &str) -> PathBuf {
    let mut path = directory.join(program);
    if !program.contains('.') {
        path.set_extension("exe");
    }
    path
}

#[cfg(not(windows))]
fn executable_path(directory: &Path, program: &str) -> PathBuf {
    directory.join(program)
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    if !path.is_file() {
        return false;
    }
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a live, NUL-terminated C string. `faccessat` does not
    // retain the pointer. `AT_EACCESS` checks access with effective IDs.
    unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0 }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
