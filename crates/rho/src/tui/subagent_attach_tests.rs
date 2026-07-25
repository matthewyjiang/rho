use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;

use super::*;
use crate::herdr::HerdrReporter;

#[test]
fn attach_command_is_stable_and_unquoted() {
    assert_eq!(attach_command("a1b2c3"), "rho attach a1b2c3");
}

#[test]
fn pane_id_parses_herdr_split_response() {
    let stdout =
        br#"{"id":"cli:pane:split","result":{"pane":{"pane_id":"wAN:p3"},"type":"pane_info"}}"#;
    assert_eq!(
        pane_id_from_split_response(stdout).as_deref(),
        Some("wAN:p3")
    );
}

#[test]
fn pane_id_rejects_malformed_split_response() {
    assert_eq!(pane_id_from_split_response(b"not-json"), None);
    assert_eq!(
        pane_id_from_split_response(br#"{"result":{"pane":{}}}"#),
        None
    );
}

#[test]
fn destination_is_clipboard_without_herdr() {
    assert_eq!(
        destination(&HerdrReporter::default()),
        AttachDestination::Clipboard
    );
}

#[test]
fn destination_uses_herdr_pane_when_configured() {
    let herdr = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        "HERDR_PANE_ID" => Some("w1:p1".into()),
        _ => None,
    });
    #[cfg(unix)]
    assert_eq!(
        destination(&herdr),
        AttachDestination::HerdrPane {
            pane_id: "w1:p1".into()
        }
    );
    #[cfg(not(unix))]
    assert_eq!(destination(&herdr), AttachDestination::Clipboard);
}

#[test]
fn action_hint_matches_destination() {
    assert_eq!(action_hint(&AttachDestination::Clipboard), "copy attach");
    assert_eq!(
        action_hint(&AttachDestination::HerdrPane {
            pane_id: "1-1".into()
        }),
        "open pane"
    );
}

#[test]
fn shell_quoting_wraps_paths_with_spaces() {
    assert!(needs_shell_quoting("/tmp/my rho/rho"));
    assert_eq!(shell_single_quote("a'b"), r"'a'\''b'");
}

#[derive(Default)]
struct RecordingPaneOpener {
    opens: Arc<Mutex<Vec<(String, String)>>>,
    fail: bool,
}

impl SubagentPaneOpener for RecordingPaneOpener {
    fn open(&mut self, pane_id: &str, command: &str) -> io::Result<()> {
        self.opens
            .lock()
            .unwrap()
            .push((pane_id.to_string(), command.to_string()));
        if self.fail {
            Err(io::Error::other("boom"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
struct RecordingClipboard {
    copied: Arc<Mutex<Vec<String>>>,
}

impl super::super::clipboard::ClipboardWriter for RecordingClipboard {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome> {
        self.copied.lock().unwrap().push(text.to_string());
        Ok(CopyOutcome::Confirmed)
    }
}

#[test]
fn activate_copies_attach_command_outside_herdr() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = crate::tui::tests::test_app();
    app.clipboard = Box::new(RecordingClipboard {
        copied: Arc::clone(&copied),
    });

    let target = SubagentAttachTarget {
        row: 0,
        run_id: "a1b2c3".into(),
        agent_id: "explorer".into(),
    };
    app.activate_subagent_row(&target, Instant::now());

    assert_eq!(copied.lock().unwrap().as_slice(), ["rho attach a1b2c3"]);
    assert_eq!(
        app.history.last_status_notice(),
        Some("copied attach command: rho attach a1b2c3")
    );
}

#[cfg(unix)]
#[test]
fn activate_opens_herdr_pane_when_configured() {
    let opens = Arc::new(Mutex::new(Vec::new()));
    let mut app = crate::tui::tests::test_app();
    app.info.services.herdr = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        "HERDR_PANE_ID" => Some("w1:p1".into()),
        _ => None,
    });
    app.pane_opener = Box::new(RecordingPaneOpener {
        opens: Arc::clone(&opens),
        fail: false,
    });

    let target = SubagentAttachTarget {
        row: 0,
        run_id: "a1b2c3".into(),
        agent_id: "explorer".into(),
    };
    app.activate_subagent_row(&target, Instant::now());

    let opens = opens.lock().unwrap();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0].0, "w1:p1");
    assert!(
        opens[0].1.ends_with(" attach a1b2c3"),
        "unexpected pane command: {}",
        opens[0].1
    );
    assert_eq!(
        app.history.last_status_notice(),
        Some("opened a herdr pane attached to explorer a1b2c3")
    );
}

#[cfg(unix)]
#[test]
fn activate_falls_back_to_clipboard_when_herdr_open_fails() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = crate::tui::tests::test_app();
    app.info.services.herdr = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        "HERDR_PANE_ID" => Some("w1:p1".into()),
        _ => None,
    });
    app.clipboard = Box::new(RecordingClipboard {
        copied: Arc::clone(&copied),
    });
    app.pane_opener = Box::new(RecordingPaneOpener {
        opens: Arc::new(Mutex::new(Vec::new())),
        fail: true,
    });

    let target = SubagentAttachTarget {
        row: 0,
        run_id: "a1b2c3".into(),
        agent_id: "explorer".into(),
    };
    app.activate_subagent_row(&target, Instant::now());

    assert_eq!(copied.lock().unwrap().as_slice(), ["rho attach a1b2c3"]);
    assert_eq!(
        app.history.last_status_notice(),
        Some("copied attach command: rho attach a1b2c3")
    );
}
