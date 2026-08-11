use pretty_assertions::assert_eq;

use super::*;

#[cfg(unix)]
use std::process::Stdio;

/// Covers: a chatty child cannot grow the stderr capture without bound, and the
/// kept slice is the tail, marked as elided.
/// Owner: the drain's stderr capture, which is the only bound on that memory.
#[test]
fn stderr_capture_keeps_a_bounded_tail() {
    let mut tail = StderrTail::default();
    for _ in 0..64 {
        tail.push(&[b'a'; MAX_STDERR_BYTES]);
        assert!(
            tail.bytes.len() <= MAX_STDERR_BYTES,
            "capture grew past its budget"
        );
    }
    tail.push(b"last line\n");

    let text = tail.finish();
    assert!(text.starts_with(rho_sdk::ELLIPSIS), "elision is not marked");
    assert!(
        text.ends_with("last line"),
        "kept the head instead of the tail"
    );
}

/// Covers: cutting the head mid-character never opens the tail on a replacement
/// character.
/// Owner: the byte-level boundary walk, which `String::from_utf8_lossy` cannot
/// recover from once the cut has happened.
#[test]
fn stderr_capture_cuts_on_a_character_boundary() {
    let mut tail = StderrTail::default();
    // Three-byte characters make every cut land inside one unless it is walked
    // forward: the budget is not a multiple of three.
    tail.push("★".repeat(MAX_STDERR_BYTES).as_bytes());

    let text = tail.finish();
    assert_eq!(text.matches('\u{FFFD}').count(), 0);
}

/// Covers: stderr short enough to keep whole is reported without an elision
/// marker.
/// Owner: the same capture, whose no-elision path feeds one-shot failure text.
#[test]
fn stderr_capture_keeps_short_output_whole() {
    let mut tail = StderrTail::default();
    tail.push(b"  boom\n");

    assert_eq!(tail.finish(), "boom");
}

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
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        drain_child(
            &mut child,
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

    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain_child(
            &mut child,
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

    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain_child(
            &mut child,
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
