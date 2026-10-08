use base64::Engine as _;
use pretty_assertions::assert_eq;

use super::{
    BlockedKind, ProgramStatus, ProgramStatusReporter, ProgramStatusSupport, SettledTurn, CLEAR,
};
use crate::tui::theme_terminal::{has_device_attributes_reply, parse_probe_response};

fn b64(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

fn report(body: &str) -> Vec<u8> {
    format!("\x1b]7501;{body}\x1b\\").into_bytes()
}

// Covers: the OSC 7501 wire format, including the message rules terminals
// enforce by discarding the whole report (one line, no control characters,
// 2048 decoded bytes).
// Owner: program status encoding.
#[test]
fn encodes_root_record_reports() {
    let long = "é".repeat(1500);
    let cases = [
        (ProgramStatus::Idle, report("state=idle:app=rho")),
        (ProgramStatus::Working, report("state=working:app=rho")),
        (ProgramStatus::Done, report("state=done:app=rho")),
        (
            ProgramStatus::Blocked {
                kind: Some(BlockedKind::Permission),
                message: "waiting for approval".into(),
            },
            report(&format!(
                "state=blocked:app=rho:kind=permission:msg={}",
                b64("waiting for approval")
            )),
        ),
        (
            ProgramStatus::Blocked {
                kind: None,
                message: "  \n".into(),
            },
            report("state=blocked:app=rho"),
        ),
        (
            ProgramStatus::Error {
                message: "\n  rate\tlimited\x1b[31m \u{9b}x\nsecond line".into(),
            },
            report(&format!(
                "state=error:app=rho:msg={}",
                b64("rate limited [31m  x")
            )),
        ),
        (
            ProgramStatus::Error {
                message: long.clone(),
            },
            // 1024 two-byte characters fill the cap without splitting one.
            report(&format!("state=error:app=rho:msg={}", b64(&long[..2048]))),
        ),
    ];
    for (status, expected) in cases {
        assert_eq!(
            String::from_utf8(status.encode()).unwrap(),
            String::from_utf8(expected).unwrap(),
            "{status:?}"
        );
    }
}

// Covers: support comes only from a 7501 reply that precedes the device
// attributes sentinel; the env override wins either way.
// Owner: startup terminal probe.
#[test]
fn detects_support_from_probe_replies() {
    use ProgramStatusSupport::{Supported, Unsupported};
    const DA1: &str = "\x1b[?62;22c";
    const PALETTE: &str = "\x1b]11;rgb:0000/0000/0000\x1b\\";
    let cases = [
        (
            "supporting terminal",
            format!("\x1b]7501;?\x1b\\{PALETTE}{DA1}"),
            Supported,
        ),
        ("BEL terminator", format!("\x1b]7501;?\x07{DA1}"), Supported),
        (
            "future reply pairs",
            format!("\x1b]7501;?v=2\x1b\\{DA1}"),
            Supported,
        ),
        (
            "no reply before DA1",
            format!("{PALETTE}{DA1}"),
            Unsupported,
        ),
        (
            "other OSC 7501 body",
            format!("\x1b]7501;state=idle\x1b\\{DA1}"),
            Unsupported,
        ),
    ];
    for (name, response, expected) in cases {
        assert_eq!(
            parse_probe_response(&response).program_status,
            expected,
            "{name}"
        );
        assert!(has_device_attributes_reply(&response), "{name}");
    }
    assert!(!has_device_attributes_reply("\x1b]7501;?\x1b\\\x1b[?1u"));

    let overrides = [
        (Unsupported, Some("1"), Supported),
        (Supported, Some("0"), Unsupported),
        (Supported, Some("auto"), Supported),
        (Unsupported, None, Unsupported),
    ];
    for (probed, value, expected) in overrides {
        assert_eq!(
            probed.with_override(value),
            expected,
            "{probed:?} {value:?}"
        );
    }
}

// Covers: repeated statuses are not re-sent, unsupported terminals get
// nothing, and exit clears only a record Rho created.
// Owner: program status reporter.
#[test]
fn reporter_skips_repeats_and_clears_once() {
    let mut unsupported = ProgramStatusReporter::new(ProgramStatusSupport::Unsupported);
    assert_eq!(unsupported.report(ProgramStatus::Working), None);
    assert_eq!(unsupported.clear(), None);

    let mut reporter = ProgramStatusReporter::new(ProgramStatusSupport::Supported);
    assert_eq!(reporter.clear(), None);
    assert_eq!(
        reporter.report(ProgramStatus::Working),
        Some(report("state=working:app=rho"))
    );
    assert_eq!(reporter.report(ProgramStatus::Working), None);
    reporter.turn_settled(SettledTurn::Failed {
        message: "boom".into(),
    });
    let settled = reporter.settled_status();
    assert_eq!(
        settled,
        ProgramStatus::Error {
            message: "boom".into()
        }
    );
    assert!(reporter.report(settled).is_some());
    assert_eq!(reporter.clear(), Some(CLEAR.to_vec()));
    assert_eq!(reporter.clear(), None);
}
