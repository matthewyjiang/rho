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

// Covers: empty native clipboard is a successful no-op paste, not an error toast.
// Owner: clipboard text paste
#[test]
fn empty_native_clipboard_pastes_as_empty_string() {
    assert_eq!(
        clipboard_text_from_native(Err(arboard::Error::ContentNotAvailable)).unwrap(),
        ""
    );
}

// Covers: text paste backends follow session policy (remote cannot read).
// Owner: clipboard text paste
#[test]
fn text_paste_follows_session_backends() {
    struct Case {
        session: SessionKind,
        native: Result<&'static str, &'static str>,
        windows: Result<&'static str, &'static str>,
        expected: Result<&'static str, io::ErrorKind>,
    }

    let cases = [
        Case {
            session: SessionKind::Remote,
            native: Ok("native"),
            windows: Ok("windows"),
            expected: Err(io::ErrorKind::Unsupported),
        },
        Case {
            session: SessionKind::Local,
            native: Ok("native"),
            windows: Err("unused"),
            expected: Ok("native"),
        },
        Case {
            session: SessionKind::Wsl,
            native: Ok("native"),
            windows: Ok("windows"),
            expected: Ok("windows"),
        },
        Case {
            session: SessionKind::Wsl,
            native: Ok("native"),
            windows: Err("powershell missing"),
            expected: Ok("native"),
        },
        Case {
            session: SessionKind::Wsl,
            native: Err("native failed"),
            windows: Err("powershell missing"),
            expected: Err(io::ErrorKind::Other),
        },
    ];

    for case in cases {
        let result = paste_text_with(
            case.session,
            || match case.native {
                Ok(text) => Ok(text.to_string()),
                Err(message) => Err(io::Error::other(message)),
            },
            || match case.windows {
                Ok(text) => Ok(text.to_string()),
                Err(message) => Err(io::Error::other(message)),
            },
        );
        match (result, case.expected) {
            (Ok(text), Ok(expected)) => assert_eq!(text, expected),
            (Err(error), Err(kind)) => {
                assert_eq!(error.kind(), kind);
                if case.native.is_err() && case.windows.is_err() {
                    assert_eq!(error.to_string(), "powershell missing; native failed");
                }
            }
            (Ok(text), Err(kind)) => panic!("expected {kind:?}, got {text:?}"),
            (Err(error), Ok(expected)) => panic!("expected {expected:?}, got {error}"),
        }
    }
}

// Covers: a UTF-8 BOM on host stdout is not pasted as U+FEFF.
// Owner: clipboard text paste
#[test]
fn windows_host_paste_strips_utf8_bom() {
    assert_eq!(clipboard_text_from_host_bytes(b"hello"), "hello");
    assert_eq!(
        clipboard_text_from_host_bytes(&[0xEF, 0xBB, 0xBF, b'h', b'i']),
        "hi"
    );
    assert_eq!(clipboard_text_from_host_bytes(&[0xEF, 0xBB, 0xBF]), "");
}
