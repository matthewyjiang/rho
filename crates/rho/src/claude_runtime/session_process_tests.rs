use super::*;
use pretty_assertions::assert_eq;
use std::{os::unix::fs::PermissionsExt, path::Path};

use crate::run_artifacts::AttachmentEvent;

fn write_fake_claude(path: &Path, body: &str) {
    // Fresh inode via tempfile rename; avoids overwriting a live text image.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".claude-install-{}-{}",
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
        .join("src/claude_runtime/fixtures")
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
cat >/dev/null
cat {payload}
exit {exit_code}
"#,
        payload = shell_quote(&payload_path),
    );
    write_fake_claude(bin, &script);
}

async fn run_with_fake(
    output: &Path,
    cwd: &Path,
    fake: &Path,
    max_turns: u64,
    permission_mode: PermissionMode,
    cancellation: RunCancellation,
) {
    // Keep rate-limit persistence off the host home directory without
    // mutating process env (unsafe under concurrent tests).
    let rate_limit_dir = tempfile::tempdir().unwrap();
    let rate_limit_state_path = rate_limit_dir.path().join("rate-limits.json");
    run_session(ClaudeSessionRequest {
        system_prompt: system_prompt(),
        identity: ClaudeRunIdentity {
            agent_id: "claude-planner".into(),
            agent_fingerprint: "fp".into(),
            model: Some("opus".into()),
        },
        model: Some("opus".into()),
        tools: vec!["Read".into()],
        inherit_claude_config: false,
        max_turns,
        effort: None,
        prompt: "hi".into(),
        output_file: output.to_path_buf(),
        cwd: cwd.to_path_buf(),
        permission_mode,
        cancellation,
        status_tx: None,
        started_status: None,
        overrides: ClaudeSessionOverrides {
            executable: Some(ClaudeExecutable::from_path(fake)),
            frozen_argv: None,
            auth_status: Some(Ok(logged_in())),
            rate_limit_state_path: Some(rate_limit_state_path),
            before_spawn: None,
        },
    })
    .await
    .unwrap();
    // Keep the temp root alive through the session await above.
    drop(rate_limit_dir);
}

async fn run_with_fake_prompt(
    output: &Path,
    cwd: &Path,
    fake: &Path,
    prompt: &str,
    cancellation: RunCancellation,
) {
    let rate_limit_dir = tempfile::tempdir().unwrap();
    let rate_limit_state_path = rate_limit_dir.path().join("rate-limits.json");
    run_session(ClaudeSessionRequest {
        system_prompt: system_prompt(),
        identity: ClaudeRunIdentity {
            agent_id: "claude-planner".into(),
            agent_fingerprint: "fp".into(),
            model: Some("opus".into()),
        },
        model: Some("opus".into()),
        tools: vec!["Read".into()],
        inherit_claude_config: false,
        max_turns: 8,
        effort: None,
        prompt: prompt.into(),
        output_file: output.to_path_buf(),
        cwd: cwd.to_path_buf(),
        permission_mode: PermissionMode::Auto,
        cancellation,
        status_tx: None,
        started_status: None,
        overrides: ClaudeSessionOverrides {
            executable: Some(ClaudeExecutable::from_path(fake)),
            frozen_argv: None,
            auth_status: Some(Ok(logged_in())),
            rate_limit_state_path: Some(rate_limit_state_path),
            before_spawn: None,
        },
    })
    .await
    .unwrap();
    drop(rate_limit_dir);
}

#[tokio::test]
async fn supervised_permission_mode_fails_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    write_fake_claude(&fake, "#!/bin/sh\necho 'should not spawn' >&2\nexit 1\n");
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Supervised,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("Supervised") || error.contains("supervised"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn success_stream_and_exit_zero_writes_ok() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(&fake, &fixture("success.ndjson"), 0);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.result.as_deref(), Some("Hello from Claude."));
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("sess-success-001")
    );
    assert_eq!(status.turns, 1);
    assert!(status.input_tokens.unwrap_or(0) > 0);
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

#[tokio::test]
async fn live_tool_roundtrip_stream_writes_session_and_tool_events() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(&fake, &fixture("live_tool_roundtrip.ndjson"), 0);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.result.as_deref(), Some("rho-tool-fixture-marker-42"));
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    assert_eq!(status.turns, 2);
    assert_eq!(status.input_tokens, Some(4 + 14452 + 5604));
    assert_eq!(status.output_tokens, Some(102));
    assert_eq!(status.error, None);

    let events = read_attachment_events(&output);
    assert!(
        events.iter().any(|event| matches!(
            event,
            AttachmentEvent::ToolStarted { card, .. }
                if card.header_text().contains("Read") || card.facts.iter().any(|f| f.plain_text().contains("Read")) || card.body.plain_lines().iter().any(|line| line.contains("Read"))
        )),
        "tool started: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AttachmentEvent::ToolFinished { card, .. } if card.status == rho_tools::tool_card::ToolStatus::Ok
        )),
        "tool finished: {events:?}"
    );
    let assistant_text: String = events
        .iter()
        .filter_map(|event| match event {
            AttachmentEvent::AssistantTextDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        assistant_text.contains("rho-tool-fixture-marker-42"),
        "assistant text: {assistant_text:?} events: {events:?}"
    );
    assert_eq!(count_terminal_events(&events), 1);
    assert!(events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Completed)));
}

#[tokio::test]
async fn success_stream_with_nonzero_exit_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(&fake, &fixture("success.ndjson"), 2);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("exited with") || error.contains("exit"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn failure_terminal_result_is_error_even_on_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(&fake, &fixture("error_result.ndjson"), 0);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("hit max turns") || error.contains("error_max_turns"),
        "unexpected error: {error}"
    );
    let events = read_attachment_events(&output);
    assert_eq!(
        count_terminal_events(&events),
        1,
        "exactly one terminal Failed"
    );
    assert!(events.iter().any(|event| {
        matches!(event, AttachmentEvent::Failed(text) if text.contains("hit max turns"))
    }));
}

#[tokio::test]
async fn success_result_with_nonzero_exit_emits_one_failed_not_completed() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(&fake, &fixture("success.ndjson"), 2);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let events = read_attachment_events(&output);
    assert_eq!(count_terminal_events(&events), 1);
    assert!(events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Failed(_))));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Completed)));
}

#[tokio::test]
async fn protocol_type_error_emits_one_failed_overall() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(
        &fake,
        r#"{"type":"system","subtype":"init","session_id":"sess-err"}
{"type":"error","result":"protocol boom"}
"#,
        0,
    );
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let events = read_attachment_events(&output);
    assert_eq!(
        count_terminal_events(&events),
        1,
        "protocol error must not double-Failed with exit finalize"
    );
    assert!(events.iter().any(|event| {
        matches!(event, AttachmentEvent::Failed(text) if text.contains("protocol boom"))
    }));
}

#[tokio::test]
async fn missing_terminal_result_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(
        &fake,
        r#"{"type":"system","subtype":"init","session_id":"sess-x"}
{"type":"assistant","session_id":"sess-x","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"hi"}]}}
"#,
        0,
    );
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("without a terminal result"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn invalid_terminal_fields_are_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    install_streaming_fake(
        &fake,
        r#"{"type":"result","result":"maybe","session_id":"sess-invalid"}
"#,
        0,
    );
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("missing subtype") || error.contains("invalid"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn invalid_utf8_stdout_fails_run() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    let payload = dir.path().join("bad.bin");
    std::fs::write(&payload, [0xff, b'\n']).unwrap();
    let script = format!(
        r#"#!/bin/sh
cat >/dev/null
cat {}
exit 0
"#,
        shell_quote(&payload)
    );
    write_fake_claude(&fake, &script);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("UTF-8") || error.contains("utf-8") || error.contains("malformed"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn oversize_line_fails_run() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    let payload = dir.path().join("big.ndjson");
    let mut bytes = vec![b'a'; crate::claude_runtime::line_decoder::MAX_NDJSON_LINE_BYTES + 8];
    bytes.push(b'\n');
    std::fs::write(&payload, bytes).unwrap();
    let script = format!(
        r#"#!/bin/sh
cat >/dev/null
cat {}
exit 0
"#,
        shell_quote(&payload)
    );
    write_fake_claude(&fake, &script);
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("oversize") || error.contains("exceeds"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn max_turns_unsupported_stderr_is_diagnosed() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    write_fake_claude(
        &fake,
        r#"#!/bin/sh
cat >/dev/null
echo "error: unknown option '--max-turns'" >&2
exit 2
"#,
    );
    run_with_fake(
        &output,
        dir.path(),
        &fake,
        8,
        PermissionMode::Auto,
        RunCancellation::new(),
    )
    .await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap_or_default();
    assert!(
        error.contains("max-turns") || error.contains("--max-turns"),
        "unexpected error: {error}"
    );
}

/// Child floods stdout before reading stdin. Old ordering awaited the full
/// prompt write before draining stdout, so a filled pipe deadlocked.
#[tokio::test]
async fn concurrent_stdin_write_drains_high_volume_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let fake = dir.path().join("claude");
    let payload = dir.path().join("flood.ndjson");
    // ~1 MiB of NDJSON lines, enough to fill a typical pipe buffer.
    let line = r#"{"type":"assistant","session_id":"s","message":{"id":"m","role":"assistant","content":[{"type":"text","text":"padpadpadpadpadpadpadpadpadpadpadpadpadpadpadpad"}]}}"#;
    let mut body = String::with_capacity(1024 * 1024 + 256);
    while body.len() < 1024 * 1024 {
        body.push_str(line);
        body.push('\n');
    }
    body.push_str(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"drained-while-writing","session_id":"s","num_turns":1,"usage":{"input_tokens":1,"output_tokens":1}}"#,
    );
    body.push('\n');
    std::fs::write(&payload, body).unwrap();

    // Emit a large stdout payload first; only then consume stdin. The parent
    // must drain while still writing the prompt or the pipes wedge.
    let script = format!(
        r#"#!/bin/sh
cat {payload}
cat >/dev/null
exit 0
"#,
        payload = shell_quote(&payload)
    );
    write_fake_claude(&fake, &script);

    // Large prompt so stdin write itself needs multiple pipe buffers.
    let prompt = "P".repeat(256 * 1024);
    tokio::time::timeout(Duration::from_secs(5), async {
        run_with_fake_prompt(&output, dir.path(), &fake, &prompt, RunCancellation::new()).await;
    })
    .await
    .expect("draining stdout while writing stdin must not deadlock");

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.result.as_deref(), Some("drained-while-writing"));
}
