use pretty_assertions::assert_eq;

use super::super::line_decoder::MAX_NDJSON_LINE_BYTES;
use super::super::stream::StreamMapper;
use super::*;

fn claude_drain_config() -> DrainConfig {
    DrainConfig {
        program_label: "claude code",
        max_line_bytes: MAX_NDJSON_LINE_BYTES,
    }
}

#[cfg(unix)]
use std::process::Stdio;

/// Covers: a descendant that inherits a captured pipe cannot keep a completed
/// Claude run open forever.
/// Owner: the shared child drain, which must reap the leader before inherited
/// descriptors can reach EOF.
#[cfg(unix)]
#[tokio::test]
async fn child_exit_closes_pipes_inherited_by_descendants() {
    let mut command = tokio::process::Command::new("sh");
    command
        // The descendant holds both output descriptors for 30 seconds unless
        // the drain observes the shell exit and cleans up its process group.
        .args(["-c", "sleep 30 &"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = OwnedChild::spawn(command).expect("spawn inherited-pipe fixture");
    let cancellation = CancellationToken::new();
    let mut on_effect = |_| {};

    // Three seconds is a generous CI tripwire for a process that exits at once,
    // while remaining far below the fixture descendant's 30-second lifetime.
    let mut mapper = StreamMapper::new();
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        drain_child(
            &mut child,
            claude_drain_config(),
            &mut mapper,
            DrainInput::Text {
                prompt: String::new(),
            },
            &cancellation,
            &mut on_effect,
        ),
    )
    .await
    .expect("drain waited for an inherited descriptor");

    assert!(matches!(drained.end, DrainEnd::Exited(Ok(status)) if status.success()));
}

/// Covers: a child that exits before consuming stdin must not surface as a bare
/// broken-pipe write failure; exit status and stderr remain available for
/// diagnosis (for example unsupported `--max-turns`).
/// Owner: the shared child drain's stdin concurrent write path.
#[cfg(unix)]
#[tokio::test]
async fn broken_pipe_on_stdin_still_reaps_exit_and_stderr() {
    let mut command = tokio::process::Command::new("sh");
    command
        // Exit without reading stdin so the parent's prompt write hits EPIPE.
        // A large prompt makes the race reliable across slow CI hosts.
        .args([
            "-c",
            "echo \"error: unknown option '--max-turns'\" >&2; exit 2",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = OwnedChild::spawn(command).expect("spawn early-exit fixture");
    let cancellation = CancellationToken::new();
    let mut on_effect = |_| {};
    let prompt = "P".repeat(256 * 1024);

    let mut mapper = StreamMapper::new();
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain_child(
            &mut child,
            claude_drain_config(),
            &mut mapper,
            DrainInput::Text { prompt },
            &cancellation,
            &mut on_effect,
        ),
    )
    .await
    .expect("early-exit child must not hang the drain");

    assert!(
        matches!(drained.end, DrainEnd::Exited(Ok(status)) if !status.success()),
        "expected reaped non-zero exit, got {:?}",
        match &drained.end {
            DrainEnd::StdinFailed(error) => format!("StdinFailed({error})"),
            DrainEnd::StreamFailed(error) => format!("StreamFailed({error})"),
            DrainEnd::Cancelled => "Cancelled".into(),
            DrainEnd::Exited(Ok(status)) => format!("Exited(Ok({status}))"),
            DrainEnd::Exited(Err(error)) => format!("Exited(Err({error}))"),
        }
    );
    assert!(
        drained.stderr.contains("max-turns"),
        "stderr diagnosis must still be captured: {:?}",
        drained.stderr
    );
}

/// Covers: stream-json drain writes the initial user turn plus a queued parent
/// follow-up, then closes stdin after the terminal result so the child exits.
#[cfg(unix)]
#[tokio::test]
async fn stream_json_parent_message_is_written_before_close() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-claude.py");
    std::fs::write(
        &fake,
        r#"#!/usr/bin/env python3
import sys, json
line1 = sys.stdin.readline()
line2 = sys.stdin.readline()
assert line1 and line2, (line1, line2)
obj1 = json.loads(line1)
obj2 = json.loads(line2)
assert obj1["type"] == "user", obj1
assert "Message from the parent session" in obj2["message"]["content"], obj2
print(json.dumps({
    "type": "result",
    "subtype": "success",
    "is_error": False,
    "result": "got-parent",
    "session_id": "s",
    "num_turns": 2,
    "usage": {"input_tokens": 1, "output_tokens": 1},
}), flush=True)
sys.stdin.read()
"#,
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut command = tokio::process::Command::new(&fake);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = OwnedChild::spawn(command).expect("spawn stream-json fixture");
    let (handle, receiver) = super::super::messaging::message_channel();
    let cancellation = CancellationToken::new();
    let mut on_effect = |_| {};

    handle.send("pivot now".into()).await.unwrap();

    let mut mapper = StreamMapper::new();
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain_child(
            &mut child,
            claude_drain_config(),
            &mut mapper,
            DrainInput::StreamJson {
                initial_prompt: "start".into(),
                parent_messages: Some(receiver),
            },
            &cancellation,
            &mut on_effect,
        ),
    )
    .await
    .expect("drain timed out");
    match &drained.end {
        DrainEnd::Exited(Ok(status)) if status.success() => {}
        DrainEnd::Exited(Ok(status)) => {
            panic!("child exit {:?}; stderr={}", status.code(), drained.stderr)
        }
        DrainEnd::Exited(Err(error)) => panic!("wait error {error}; stderr={}", drained.stderr),
        DrainEnd::Cancelled => panic!("cancelled; stderr={}", drained.stderr),
        DrainEnd::StdinFailed(error) | DrainEnd::StreamFailed(error) => {
            panic!("{error}; stderr={}", drained.stderr)
        }
    }
    let terminal = drained.terminal.expect("terminal result");
    assert_eq!(terminal.result_text.as_deref(), Some("got-parent"));
}
