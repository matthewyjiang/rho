use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use pretty_assertions::assert_eq;

use super::*;
use crate::herdr::HerdrReporter;

#[test]
fn attach_command_is_stable_and_unquoted() {
    assert_eq!(attach_command("a1b2c3"), "rho attach a1b2c3");
}

#[test]
fn pane_command_quotes_every_shell_argument() {
    assert_eq!(
        pane_attach_command_for_executable(Some(Path::new("/tmp/rho`*?[x]")), "a1b2c3"),
        "'/tmp/rho`*?[x]' 'attach' 'a1b2c3'"
    );
    assert_eq!(
        pane_attach_command_for_executable(None, "a1b2c3"),
        "'rho' 'attach' 'a1b2c3'"
    );
    assert_eq!(shell_single_quote("a'b"), r"'a'\''b'");
}

#[test]
fn destination_is_clipboard_without_herdr() {
    assert_eq!(
        destination(&HerdrReporter::default()),
        AttachDestination::Clipboard
    );
}

#[test]
fn destination_uses_herdr_when_configured() {
    let herdr = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        "HERDR_PANE_ID" => Some("w1:p1".into()),
        _ => None,
    });
    #[cfg(unix)]
    assert_eq!(destination(&herdr), AttachDestination::HerdrPane);
    #[cfg(not(unix))]
    assert_eq!(destination(&herdr), AttachDestination::Clipboard);
}

#[test]
fn action_hint_matches_destination() {
    assert_eq!(action_hint(AttachDestination::Clipboard), "copy attach");
    assert_eq!(action_hint(AttachDestination::HerdrPane), "open pane");
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

fn target() -> SubagentAttachTarget {
    SubagentAttachTarget {
        run_id: "a1b2c3".into(),
        agent_id: "explorer".into(),
    }
}

#[test]
fn activate_copies_attach_command_outside_herdr() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = crate::tui::tests::test_app();
    app.clipboard = Box::new(RecordingClipboard {
        copied: Arc::clone(&copied),
    });

    app.activate_subagent_row(&target(), Instant::now());

    assert_eq!(copied.lock().unwrap().as_slice(), ["rho attach a1b2c3"]);
    assert_eq!(
        app.history.last_status_notice(),
        Some("copied attach command: rho attach a1b2c3")
    );
}

#[cfg(unix)]
async fn wait_for_attach_result(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.has_pending_subagent_attach() {
            if !app.poll_pending_subagent_attaches(Instant::now()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    })
    .await
    .expect("subagent attach result");
}

#[cfg(unix)]
#[tokio::test]
async fn activate_opens_herdr_pane_in_background() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = crate::herdr::test_support::TestHerdrServer::bind_with_responses(
        &socket_path,
        vec![
            br#"{"id":"1","result":{"pane":{"pane_id":"w1:p2"}}}
"#,
            br#"{"id":"2","result":{}}
"#,
        ],
    )
    .await;
    let mut app = crate::tui::tests::test_app();
    app.info.services.herdr = crate::herdr::test_support::reporter_for_socket(&socket_path);

    app.activate_subagent_row(&target(), Instant::now());

    assert!(app.has_pending_subagent_attach());
    assert_eq!(
        app.history.last_status_notice(),
        Some("opening a herdr pane for explorer a1b2c3")
    );
    assert_eq!(server.next_request().await["method"], "pane.split");
    assert_eq!(server.next_request().await["method"], "pane.send_input");
    wait_for_attach_result(&mut app).await;
    assert_eq!(
        app.history.last_status_notice(),
        Some("opened a herdr pane attached to explorer a1b2c3")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn activate_falls_back_to_clipboard_and_keeps_herdr_error() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let _server = crate::herdr::test_support::TestHerdrServer::bind_with_responses(
        &socket_path,
        vec![
            br#"{"id":"1","error":{"code":"pane_missing","message":"stale pane"}}
"#,
        ],
    )
    .await;
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = crate::tui::tests::test_app();
    app.info.services.herdr = crate::herdr::test_support::reporter_for_socket(&socket_path);
    app.clipboard = Box::new(RecordingClipboard {
        copied: Arc::clone(&copied),
    });

    app.activate_subagent_row(&target(), Instant::now());
    wait_for_attach_result(&mut app).await;

    assert_eq!(copied.lock().unwrap().as_slice(), ["rho attach a1b2c3"]);
    assert_eq!(
        app.history.last_status_notice(),
        Some("herdr pane failed (stale pane); copied attach command: rho attach a1b2c3")
    );
}
