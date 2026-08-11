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
        drain_child(&mut child, "", &cancellation, &mut on_effect),
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
        drain_child(&mut child, &prompt, &cancellation, &mut on_effect),
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
