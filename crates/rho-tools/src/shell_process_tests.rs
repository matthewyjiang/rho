use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::*;

#[test]
fn shell_args_reject_invalid_payload() {
    assert!(
        ShellArgs::parse(json!({"timeout_seconds": 1})).is_err(),
        "command is required"
    );
}

// Covers: timeout errors must keep partial streams for diagnosis
// Owner: pure unit (shell process)
#[test]
fn timeout_error_includes_partial_output() {
    let err = timeout_error(
        b"started",
        b"noise",
        Duration::from_secs(3),
        /*max_output_bytes*/ 12_000,
    );
    let message = err.to_string();
    assert!(message.contains("timed out after 3s"));
    assert!(message.contains("started"));
    assert!(message.contains("noise"));
}

#[test]
fn timeout_error_respects_output_budget() {
    let err = timeout_error(
        &[b'x'; 200],
        b"",
        Duration::from_secs(1),
        /*max_output_bytes*/ 40,
    );
    assert!(err.to_string().contains("[truncated]"));
}

#[cfg(unix)]
#[test]
fn finished_result_respects_output_budget() {
    use std::os::unix::process::ExitStatusExt;

    let status = std::process::ExitStatus::from_raw(0);
    let result = finished_result(
        "call_1".into(),
        status,
        &[b'y'; 200],
        b"",
        Duration::from_secs(0),
        /*max_output_bytes*/ 40,
    );
    assert!(result.ok);
    assert!(result.content.contains("[truncated]"));
}

#[cfg(unix)]
#[test]
fn finished_result_uses_signal_when_exit_code_is_absent() {
    use std::os::unix::process::ExitStatusExt;

    // Wait status for termination by signal 9 (SIGKILL).
    let status = std::process::ExitStatus::from_raw(9);
    let result = finished_result(
        "call_1".into(),
        status,
        b"out",
        b"err",
        Duration::from_secs(0),
        /*max_output_bytes*/ 12_000,
    );
    assert!(!result.ok);
    assert!(result.content.contains("exit code: signal"));
}

// Covers: retained stream bytes must stop at the configured budget
// Owner: pure unit (shell process)
#[test]
fn stream_session_caps_retained_stdout_and_stderr() {
    let (_tx, chunk_rx) = tokio::sync::mpsc::channel(4);
    let mut streams = StreamSession {
        chunk_rx,
        readers: Vec::new(),
        stdout: Vec::new(),
        stderr: Vec::new(),
        retained_bytes: 0,
        max_output_bytes: 10,
        output_open: true,
    };

    streams.apply_chunk(Some((StreamKind::Stdout, b"hello-world".to_vec())));
    streams.apply_chunk(Some((StreamKind::Stderr, b"more".to_vec())));
    streams.apply_chunk(Some((StreamKind::Stdout, b"extra".to_vec())));

    assert_eq!(streams.stdout, b"hello-worl");
    assert!(streams.stderr.is_empty());
    assert_eq!(streams.retained_bytes, 10);
    assert_eq!(
        streams.stdout.len() + streams.stderr.len(),
        streams.max_output_bytes
    );
}

// Covers: dropped consumers must stop the reader instead of blocking writers
// Owner: pure unit (shell process)
#[tokio::test]
async fn read_stream_stops_when_consumer_disconnects() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    drop(rx);

    let (mut writer, reader) = tokio::io::duplex(1024);
    let reader_task = tokio::spawn(read_stream(StreamKind::Stdout, reader, tx));
    // Keep the write side open so the reader only exits because send fails,
    // not because the duplex peer closed.
    let writer_task = tokio::spawn(async move {
        for _ in 0..32 {
            let _ = writer.write_all(&[b'x'; 256]).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    tokio::time::timeout(Duration::from_secs(2), reader_task)
        .await
        .expect("read_stream should stop after the consumer disconnects")
        .expect("reader task should finish cleanly");
    writer_task.abort();
}

/// Reader that yields one chunk and then fails, standing in for a pipe that
/// breaks mid-command.
struct FailingReader {
    delivered: bool,
}

impl tokio::io::AsyncRead for FailingReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.delivered {
            return std::task::Poll::Ready(Err(std::io::Error::other("pipe exploded")));
        }
        self.delivered = true;
        buf.put_slice(b"partial");
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn stream_read_failure_is_reported_on_stderr() {
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel(4);
    read_stream(
        StreamKind::Stdout,
        FailingReader { delivered: false },
        chunk_tx,
    )
    .await;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    while let Some((kind, bytes)) = chunk_rx.recv().await {
        match kind {
            StreamKind::Stdout => stdout.extend(bytes),
            StreamKind::Stderr => stderr.extend(bytes),
        }
    }

    assert_eq!(String::from_utf8(stdout).unwrap(), "partial");
    assert_eq!(
        String::from_utf8(stderr).unwrap(),
        "\n[rho: stdout capture ended early: pipe exploded]\n"
    );
}

#[test]
fn shell_content_preserves_signal_exit_status() {
    let parsed =
        parse_shell_content("stdout:\nout\n\nstderr:\nerr\n\ntime: 1.5s  exit code: signal");
    assert_eq!(parsed.exit_code, None);
    assert_eq!(parsed.exit_status.as_deref(), Some("signal"));
    assert_eq!(parsed.duration_ms, Some(1500));
}

// Covers: timeout notices must not swallow nested stderr sections
// Owner: pure unit (shell process parser)
#[test]
fn shell_content_parses_timeout_notice() {
    let parsed = parse_shell_content(
        "command timed out after 5s\n\nstdout:\na\n\nstderr:\nb\n\nstderr:\nwarning",
    );
    assert_eq!(parsed.notice.as_deref(), Some("command timed out after 5s"));
    assert_eq!(parsed.stdout, "a\n\nstderr:\nb");
}
