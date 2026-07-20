use std::io;

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn utf16_le_payload_starts_with_bom_and_encodes_text() {
    assert_eq!(
        utf16_le_bom_bytes("Ab"),
        vec![0xFF, 0xFE, b'A', 0x00, b'b', 0x00]
    );
}

#[test]
fn fallback_reports_terminal_success_as_unconfirmed() {
    let outcome = fallback_to_terminal(Ok(()), Some(io::Error::other("native failed"))).unwrap();
    assert_eq!(outcome, CopyOutcome::SentToTerminal);
}

#[test]
fn fallback_preserves_host_error_when_terminal_also_fails() {
    let error = fallback_to_terminal(
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")),
        Some(io::Error::other("native failed")),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(
        error.to_string(),
        "terminal closed (host clipboard: native failed)"
    );
}

#[test]
fn join_host_errors_keeps_both_messages() {
    let error = join_host_errors(
        io::Error::other("clip.exe missing"),
        io::Error::other("native failed"),
    );
    assert_eq!(error.to_string(), "clip.exe missing; native failed");
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
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn write_command_stdin_fails_when_the_command_exits_nonzero() {
    let error = write_command_stdin("false", &[], b"copied text").unwrap_err();
    assert!(error.to_string().contains("false exited"));
}

#[test]
fn remote_probe_uses_osc_52() {
    let probe = probe_text_write(SessionKind::Remote);
    assert_eq!(probe.status, "osc 52");
    assert!(probe.healthy);
    assert!(probe.detail.contains("Remote session"));
}

#[test]
fn local_probe_mentions_native_or_fallback() {
    let probe = probe_text_write(SessionKind::Local);
    assert!(matches!(probe.status, "native" | "osc 52 fallback"));
    assert!(probe.healthy);
}
