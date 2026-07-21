use pretty_assertions::assert_eq;

use super::*;

#[test]
fn missing_commands_are_not_available() {
    assert!(!command_available("rho-missing-clipboard-helper"));
}

#[cfg(unix)]
#[test]
fn common_unix_commands_are_available() {
    assert!(command_available("cat"));
}

#[cfg(unix)]
#[test]
fn write_command_stdin_succeeds_when_the_command_accepts_bytes() {
    write_command_stdin("cat", &[], b"copied text").unwrap();
}

#[test]
fn write_command_stdin_fails_when_the_command_is_missing() {
    let error =
        write_command_stdin("rho-missing-clipboard-helper", &[], b"copied text").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn write_command_stdin_fails_when_the_command_exits_nonzero() {
    let error = write_command_stdin("false", &[], b"copied text").unwrap_err();
    assert!(error.to_string().contains("false exited"));
}

#[cfg(unix)]
#[test]
fn resolve_command_write_prefers_nonzero_exit_over_write_error() {
    let status = std::process::Command::new("false").status().unwrap();
    let write_result = Err(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "stdin closed",
    ));

    let error = resolve_command_write("false", status, write_result).unwrap_err();

    assert!(error.to_string().contains("false exited"));
}

#[cfg(unix)]
#[test]
fn resolve_command_write_preserves_write_error_on_clean_exit() {
    let status = std::process::Command::new("true").status().unwrap();
    let write_result = Err(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "stdin closed",
    ));

    let error = resolve_command_write("true", status, write_result).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[cfg(unix)]
#[test]
fn availability_resolves_by_path_without_executing() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    // A "helper" whose only behavior is to prove execution by creating a marker.
    let marker = dir.path().join("was-executed");
    let helper = dir.path().join("clip.exe");
    std::fs::write(&helper, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(command_available_in(
        "clip.exe",
        std::iter::once(dir.path().to_path_buf())
    ));
    assert!(
        !marker.exists(),
        "availability probe must not execute the helper"
    );
}

#[cfg(unix)]
#[test]
fn non_executable_files_are_not_available() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("helper"), b"not executable").unwrap();
    assert!(!command_available_in(
        "helper",
        std::iter::once(dir.path().to_path_buf())
    ));
}

#[cfg(unix)]
#[test]
fn availability_finds_the_command_in_a_later_path_entry() {
    use std::os::unix::fs::PermissionsExt;

    let empty = tempfile::tempdir().unwrap();
    let has_it = tempfile::tempdir().unwrap();
    let helper = has_it.path().join("pngpaste");
    std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(command_available_in(
        "pngpaste",
        [empty.path().to_path_buf(), has_it.path().to_path_buf()].into_iter()
    ));
}
