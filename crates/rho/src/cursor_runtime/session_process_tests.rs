use super::*;
use pretty_assertions::assert_eq;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::agent::CursorTool;
use crate::run_artifacts::AttachmentEvent;
use crate::subagent::RunState;

fn write_fake_cursor(path: &Path, body: &str) {
    // Fresh inode via tempfile rename; avoids overwriting a live text image.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".cursor-install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, body).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path).unwrap();
}

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/cursor_runtime/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn read_attachment_events(output: &Path) -> Vec<AttachmentEvent> {
    let path = output.with_file_name(crate::subagent::ATTACHMENT_FILE_NAME);
    let body = std::fs::read_to_string(path).unwrap_or_default();
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("attachment event json"))
        .collect()
}

fn count_terminal_events(events: &[AttachmentEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .count()
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

fn install_streaming_fake(bin: &Path, ndjson: &str, exit_code: i32) {
    let payload_path = bin.with_extension("payload.ndjson");
    std::fs::write(&payload_path, ndjson).unwrap();
    let script = format!(
        r#"#!/bin/sh
cat {payload}
exit {exit_code}
"#,
        payload = shell_quote(&payload_path),
    );
    write_fake_cursor(bin, &script);
}

async fn run_with_fake(
    output: &Path,
    cwd: &Path,
    fake: &Path,
    permission_mode: PermissionMode,
    cancellation: RunCancellation,
) {
    run_session(CursorSessionRequest {
        system_prompt: system_prompt(),
        identity: cursor_identity(),
        tools: vec![CursorTool::Read],
        prompt: "hi".into(),
        output_file: output.to_path_buf(),
        cwd: cwd.to_path_buf(),
        permission_mode,
        cancellation,
        status_tx: None,
        started_status: None,
        auth_status: Some(Ok(logged_in())),
        overrides: CliSessionOverrides {
            executable: Some(CliExecutable::from_path(fake)),
            ..CliSessionOverrides::default()
        },
    })
    .await
    .unwrap();
}

// Covers: a successful Cursor stream-json run writes Ok, session id, usage,
// and exactly one Completed attachment.
// Owner: cursor session process
#[tokio::test]
async fn success_stream_and_exit_zero_writes_ok() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("cursor-agent");
    install_streaming_fake(&fake, &fixture("live_text_thinking.ndjson"), 0);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        PermissionMode::Bypass,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(status.input_tokens, Some(14414 + 5749));
    assert_eq!(status.output_tokens, Some(347));
    assert_eq!(status.error, None);
    let events = read_attachment_events(&output);
    assert_eq!(
        count_terminal_events(&events),
        1,
        "exactly one terminal attachment"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Completed)));
}

// Covers: startup failures on stderr with exit 1 and no stdout become Error
// with the stderr tail, not a missing-terminal-result message.
// Owner: cursor session process
#[tokio::test]
async fn stderr_startup_failure_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("cursor-agent");
    write_fake_cursor(
        &fake,
        "#!/bin/sh\necho 'Cannot use this model: x' >&2\nexit 1\n",
    );
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        PermissionMode::Bypass,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("Cannot use this model: x"),
        "unexpected error: {error}"
    );
}

// Covers: Auto is refused before spawn so a fake that would leave a marker
// is never invoked.
// Owner: cursor session process
#[tokio::test]
async fn auto_permission_mode_fails_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("cursor-agent");
    let marker = dir.path().join("spawned");
    let script = format!(
        "#!/bin/sh\ntouch {}\necho should-not-spawn >&2\nexit 1\n",
        shell_quote(&marker)
    );
    write_fake_cursor(&fake, &script);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    assert!(
        !marker.exists(),
        "Auto must fail before invoking cursor-agent"
    );
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    let expected = spawn::CursorSpawnError::ApprovalUnsupported(PermissionMode::Auto).to_string();
    assert!(error.contains(&expected), "unexpected error: {error}");
}

// Covers: cancelling a live Cursor child terminates it and writes Stopped
// with a Cancelled attachment.
// Owner: cursor session process
#[tokio::test]
async fn cancellation_terminates_child() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("cursor-agent");
    let marker = dir.path().join("started");
    let script = format!(
        "#!/bin/sh\ntouch {}\nexec tail -f /dev/null\n",
        shell_quote(&marker)
    );
    write_fake_cursor(&fake, &script);
    let cancellation = RunCancellation::new();
    let run = tokio::spawn({
        let output = output.clone();
        let cwd = dir.path().to_path_buf();
        let fake = fake.clone();
        let cancellation = cancellation.clone();
        async move {
            run_with_fake(&output, &cwd, &fake, PermissionMode::Bypass, cancellation).await;
        }
    });
    // Fake binary writes the marker within milliseconds of exec; 200 × 10 ms
    // = 2 s ceiling, well above observed.
    const MARKER_WAIT_ATTEMPTS: usize = 200;
    const MARKER_WAIT_MS: u64 = 10;
    for _ in 0..MARKER_WAIT_ATTEMPTS {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(MARKER_WAIT_MS)).await;
    }
    assert!(marker.exists(), "child started");
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("cancelled session must finish")
        .expect("session task");
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Stopped);
    let events = read_attachment_events(&output);
    assert!(events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Cancelled)));
}
