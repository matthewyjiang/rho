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
fn write_command_stdin_succeeds_when_a_successful_command_closes_stdin_early() {
    // `true` exits 0 without reading stdin; a payload larger than the pipe
    // buffer guarantees a broken pipe mid-write. The successful exit wins.
    let payload = vec![b'x'; 1 << 20];
    write_command_stdin("true", &[], &payload).unwrap();
}
