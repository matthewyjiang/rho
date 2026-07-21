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
