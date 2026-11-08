use std::ffi::OsString;

use pretty_assertions::assert_eq;

use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// A `PATH`-style list of `directories`.
fn search_path<P: AsRef<std::ffi::OsStr>>(directories: impl IntoIterator<Item = P>) -> OsString {
    env::join_paths(directories).unwrap()
}

#[cfg(unix)]
#[test]
fn resolves_without_running_the_executable() {
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("clip.exe");
    std::fs::write(&helper, b"#!/bin/sh\nrm \"$0\"\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        find_on(
            "clip.exe",
            Some(search_path([directory.path()]).as_os_str())
        ),
        Some(helper.clone())
    );
    assert!(helper.exists(), "lookup must not execute the helper");
}

#[cfg(unix)]
#[test]
fn rejects_a_file_the_current_user_cannot_execute() {
    // SAFETY: `geteuid` takes no pointers and has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("helper");
    std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
    // The current user owns the file, so an other-only execute bit must not
    // grant this process access.
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o001)).unwrap();

    assert_eq!(
        find_on("helper", Some(search_path([directory.path()]).as_os_str())),
        None
    );
}

#[cfg(unix)]
#[test]
fn resolves_from_a_later_path_entry() {
    let empty = tempfile::tempdir().unwrap();
    let populated = tempfile::tempdir().unwrap();
    let helper = populated.path().join("pngpaste");
    std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        find_on(
            "pngpaste",
            Some(search_path([empty.path(), populated.path()]).as_os_str())
        ),
        Some(helper)
    );
}

/// Covers: candidates follow the `PATH` entries in order (with the Windows
/// `.exe` rule), and relative entries, which would resolve against the
/// daemon's working directory instead of the client's, are skipped (#1659).
/// Owner: `candidates_on`.
#[test]
fn candidates_skip_relative_path_entries() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("usr-bin");
    let second = root.path().join("bin");
    let path = search_path([
        first.as_path(),
        Path::new(""),
        Path::new("."),
        Path::new("relative-bin"),
        second.as_path(),
    ]);
    let program = if cfg!(windows) { "cargo.exe" } else { "cargo" };

    assert_eq!(
        candidates_on("cargo", Some(path.as_os_str())).collect::<Vec<_>>(),
        vec![first.join(program), second.join(program)]
    );
}

#[cfg(windows)]
#[test]
fn resolves_a_bare_windows_name_to_an_exe() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("powershell.exe");
    std::fs::write(&executable, b"").unwrap();

    assert_eq!(
        find_on(
            "powershell",
            Some(search_path([directory.path()]).as_os_str())
        ),
        Some(executable)
    );
}

#[cfg(windows)]
#[test]
fn bare_windows_names_require_an_exe() {
    let directory = tempfile::tempdir().unwrap();
    for extension in ["", ".cmd", ".bat"] {
        std::fs::write(directory.path().join(format!("powershell{extension}")), b"").unwrap();
    }

    assert_eq!(
        find_on(
            "powershell",
            Some(search_path([directory.path()]).as_os_str())
        ),
        None
    );
}
