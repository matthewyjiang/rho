use std::{cell::Cell, io};

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn local_session_confirms_a_native_clipboard_write() {
    let terminal_called = Cell::new(false);

    let outcome = copy_with_backends(
        SessionKind::Local,
        "copied text",
        |text| {
            assert_eq!(text, "copied text");
            Ok::<(), ()>(())
        },
        |_| {
            terminal_called.set(true);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::Confirmed);
    assert!(!terminal_called.get());
}

#[test]
fn local_session_falls_back_to_an_unconfirmed_terminal_request() {
    let outcome = copy_with_backends(
        SessionKind::Local,
        "copied text",
        |_| Err(()),
        |text| {
            assert_eq!(text, "copied text");
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::SentToTerminal);
}

#[test]
fn remote_session_bypasses_the_remote_hosts_native_clipboard() {
    let outcome = copy_with_backends(
        SessionKind::Remote,
        "copied text",
        |_| -> Result<(), ()> { panic!("remote session used the native clipboard") },
        |text| {
            assert_eq!(text, "copied text");
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::SentToTerminal);
}

#[test]
fn terminal_write_errors_are_reported_after_native_clipboard_failure() {
    let error = copy_with_backends(
        SessionKind::Local,
        "copied text",
        |_| Err(()),
        |_| Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}
