use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::*;

#[test]
fn shell_args_parse_command_and_optional_timeout() {
    let args = ShellArgs::parse(json!({
        "command": "echo hi",
        "timeout_seconds": 5
    }))
    .expect("valid shell args");
    assert_eq!(args.command, "echo hi");
    assert_eq!(args.timeout(), Some(Duration::from_secs(5)));
}

#[test]
fn shell_args_parse_without_timeout() {
    let args = ShellArgs::parse(json!({"command": "true"})).expect("valid shell args");
    assert_eq!(args.command, "true");
    assert_eq!(args.timeout(), None);
}

#[test]
fn shell_args_reject_invalid_payload() {
    assert!(
        ShellArgs::parse(json!({"timeout_seconds": 1})).is_err(),
        "command is required"
    );
}

#[test]
fn running_content_formats_both_streams() {
    assert_eq!(
        running_content(b"out", b"err"),
        "stdout:\nout\n\nstderr:\nerr\n\ntime: running"
    );
}

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
fn finished_result_reports_exit_code_and_elapsed_time() {
    use std::os::unix::process::ExitStatusExt;

    // Wait status for a normal exit with code 7 is `7 << 8`.
    let status = std::process::ExitStatus::from_raw(7 << 8);
    let result = finished_result(
        "call_1".into(),
        status,
        b"out",
        b"err",
        Duration::from_millis(1500),
        /*max_output_bytes*/ 12_000,
    );
    assert!(!result.ok);
    assert_eq!(result.id, "call_1");
    assert!(result.content.contains("exit code: 7"));
    assert!(result.content.contains("time: 1.5s"));
    assert!(result.content.contains("stdout:\nout"));
    assert!(result.content.contains("stderr:\nerr"));
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

#[tokio::test]
async fn read_stream_stops_when_consumer_disconnects() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
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
