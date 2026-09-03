use std::future::Future;
use std::pin::Pin;

use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::*;
use crate::cli_runtime::line_decoder::MAX_NDJSON_LINE_BYTES;
use crate::cli_runtime::stream_effect::{StreamEffect, TerminalClassification, TerminalResult};

fn drain_config() -> DrainConfig {
    DrainConfig {
        program_label: "cli",
        max_line_bytes: MAX_NDJSON_LINE_BYTES,
    }
}

struct IgnoreMapper;

impl StreamLineMapper for IgnoreMapper {
    fn push_line(&mut self, _line: &str) -> Vec<StreamEffect> {
        Vec::new()
    }
}

struct ResultMapper;

impl StreamLineMapper for ResultMapper {
    fn push_line(&mut self, line: &str) -> Vec<StreamEffect> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        if value.get("type").and_then(|value| value.as_str()) != Some("result") {
            return Vec::new();
        }
        vec![StreamEffect::Terminal(TerminalResult {
            classification: TerminalClassification::Success {
                subtype: "success".into(),
            },
            result_text: value
                .get("result")
                .and_then(|value| value.as_str())
                .map(str::to_owned),
            error: None,
            session_id: None,
            num_turns: None,
            usage: None,
            context: None,
            total_cost_usd: None,
            permission_denials: Vec::new(),
            stop_reason: None,
        })]
    }
}

struct ChannelFollowUps {
    receiver: mpsc::Receiver<String>,
}

impl FollowUpSource for ChannelFollowUps {
    fn try_recv(&mut self) -> Result<String, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async { self.receiver.recv().await })
    }

    fn seal(&self) {}
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
    let mut mapper = IgnoreMapper;
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        drain_child(
            &mut child,
            drain_config(),
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

    let mut mapper = IgnoreMapper;
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain_child(
            &mut child,
            drain_config(),
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
    let (tx, receiver) = mpsc::channel(8);
    let cancellation = CancellationToken::new();
    let mut on_effect = |_| {};

    tx.send("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Message from the parent session (not a new task - incorporate this into your current work):\\n\\npivot now\"}}\n".into())
        .await
        .unwrap();
    // Drop the sender so drain can close stdin after the terminal result.
    drop(tx);

    let mut mapper = ResultMapper;
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain_child(
            &mut child,
            drain_config(),
            &mut mapper,
            DrainInput::StreamJson {
                initial_line:
                    "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"start\"}}\n"
                        .into(),
                follow_ups: Some(Box::new(ChannelFollowUps { receiver })),
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
