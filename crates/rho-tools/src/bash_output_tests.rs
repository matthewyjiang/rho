use serde_json::json;

use super::*;

#[tokio::test]
async fn captures_large_final_output_burst() {
    let result = Bash::new(false)
        .call(
            json!({"command": "printf 'x%.0s' {1..100000}"}),
            ToolContext {
                cwd: std::env::temp_dir(),
                max_output_bytes: 200_000,
            },
            "call_1".into(),
        )
        .await
        .unwrap();

    let stdout = result
        .content
        .strip_prefix("stdout:\n")
        .unwrap()
        .split_once("\n\nstderr:")
        .unwrap()
        .0;
    assert_eq!(stdout.len(), 100_000);
}

#[tokio::test]
async fn returns_after_shell_exits_with_background_pipe_holder() {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        Bash::new(false).call(
            json!({"command": "sleep 60 & printf done"}),
            ToolContext {
                cwd: std::env::temp_dir(),
                max_output_bytes: 12_000,
            },
            "call_1".into(),
        ),
    )
    .await
    .expect("bash call should not wait for background pipe holders")
    .unwrap();

    assert!(result.content.contains("done"));
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
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
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
