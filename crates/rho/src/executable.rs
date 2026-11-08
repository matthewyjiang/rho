use std::{
    env,
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

/// Finds a bare executable name on this process's `PATH` without spawning
/// it; see [`find_on`].
pub(crate) fn find_on_path(program: &str) -> Option<PathBuf> {
    find_on(program, env::var_os("PATH").as_deref())
}

/// Finds a bare executable name on `path`, a `PATH`-style list such as a
/// daemon session's client's (#1659), without spawning it: the first of
/// [`candidates_on`] that is an executable file. No `path` finds nothing.
pub(crate) fn find_on(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    candidates_on(program, path).find(|candidate| is_executable_file(candidate))
}

/// Where a bare executable name may be on `path`, in search order, whether
/// or not the files exist. Anything but a bare name has no candidates, and
/// neither does a missing `path`.
///
/// Relative entries, such as the empty or `.` entries a client's `PATH`
/// may carry, are skipped: they would resolve against this process's
/// working directory, which for a daemon is not the client's (#1659).
///
/// On Windows, a bare name may omit only the `.exe` extension, matching
/// `std::process::Command`. Shell-only `PATHEXT` entries such as `.cmd` and
/// `.bat` must be named explicitly.
pub(crate) fn candidates_on<'a>(
    program: &'a str,
    path: Option<&'a OsStr>,
) -> impl Iterator<Item = PathBuf> + 'a {
    let mut components = Path::new(program).components();
    let bare_name =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    path.filter(|_| bare_name)
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|directory| directory.is_absolute())
        .map(move |directory| executable_path(&directory, program))
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
