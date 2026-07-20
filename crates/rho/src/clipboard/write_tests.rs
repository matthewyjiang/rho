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

#[test]
fn remote_probe_uses_intended_osc_52_path() {
    let probe = probe_text_write_with(SessionKind::Remote, |_| false, || false);
    assert_eq!(probe.status, "osc 52");
    assert!(probe.healthy);
    assert!(probe.detail.contains("Remote session"));
}

#[test]
fn local_probe_marks_confirmed_native_as_healthy() {
    let probe = probe_text_write_with(SessionKind::Local, |_| false, || true);
    assert_eq!(probe.status, "native");
    assert!(probe.healthy);
}

#[test]
fn local_probe_marks_osc_only_as_degraded() {
    let probe = probe_text_write_with(SessionKind::Local, |_| false, || false);
    assert_eq!(probe.status, "osc 52 fallback");
    assert!(!probe.healthy);
}

#[test]
fn wsl_probe_prefers_windows_host_when_clip_exists() {
    let probe = probe_text_write_with(SessionKind::Wsl, |command| command == "clip.exe", || false);
    assert_eq!(probe.status, "windows host");
    assert!(probe.healthy);
    assert!(probe.detail.contains("clip.exe"));
}

#[test]
fn wsl_probe_uses_native_when_clip_is_missing() {
    let probe = probe_text_write_with(SessionKind::Wsl, |_| false, || true);
    assert_eq!(probe.status, "native");
    assert!(probe.healthy);
}

#[test]
fn wsl_probe_marks_osc_only_as_degraded() {
    let probe = probe_text_write_with(SessionKind::Wsl, |_| false, || false);
    assert_eq!(probe.status, "osc 52 fallback");
    assert!(!probe.healthy);
}
