use pretty_assertions::assert_eq;

use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
#[test]
fn resolves_without_running_the_executable() {
    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("clip.exe");
    std::fs::write(&helper, b"#!/bin/sh\nrm \"$0\"\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        find_in("clip.exe", std::iter::once(directory.path().to_path_buf())),
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
        find_in("helper", std::iter::once(directory.path().to_path_buf())),
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
        find_in(
            "pngpaste",
            [empty.path().to_path_buf(), populated.path().to_path_buf()]
        ),
        Some(helper)
    );
}

#[cfg(windows)]
#[test]
fn resolves_a_bare_windows_name_to_an_exe() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("powershell.exe");
    std::fs::write(&executable, b"").unwrap();

    assert_eq!(
        find_in(
            "powershell",
            std::iter::once(directory.path().to_path_buf())
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
        find_in(
            "powershell",
            std::iter::once(directory.path().to_path_buf())
        ),
        None
    );
}
