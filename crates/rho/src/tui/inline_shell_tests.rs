use super::*;

#[test]
fn parses_context_and_local_prefixes_distinctly() {
    assert_eq!(
        InlineShellMode::parse("! echo hello"),
        Some((InlineShellMode::IncludeInContext, "echo hello"))
    );
    assert_eq!(
        InlineShellMode::parse("!! echo hello"),
        Some((InlineShellMode::ExcludeFromContext, "echo hello"))
    );
    assert_eq!(InlineShellMode::parse("hello"), None);
}

#[test]
fn history_prefix_matches_mode() {
    assert_eq!(InlineShellMode::IncludeInContext.history_prefix(), "!");
    assert_eq!(InlineShellMode::ExcludeFromContext.history_prefix(), "!!");
}

#[test]
fn formats_context_with_command_and_both_streams() {
    let output = ShellOutput {
        shell: "bash".into(),
        command: "echo hello".into(),
        stdout: "hello\n".into(),
        stderr: "warning\n".into(),
        exit_code: "0".into(),
        ok: true,
    };

    let context = context_text(&output);
    assert!(context.contains("echo hello"));
    assert!(context.contains("hello\n"));
    assert!(context.contains("warning\n"));
    assert!(context.contains("exit code: 0"));
}

#[tokio::test]
async fn executes_with_selected_shell() {
    if cfg!(windows) {
        return;
    }
    let output = execute("sh", "printf inline-shell", Path::new("."))
        .await
        .unwrap();

    assert!(output.ok);
    assert_eq!(output.stdout, "inline-shell");
}

#[tokio::test]
async fn streams_output_before_command_finishes() {
    if cfg!(windows) {
        return;
    }
    let (updates_tx, mut updates_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        execute_streaming(
            "sh",
            "printf streamed; sleep 1; printf finished",
            Path::new("."),
            Some(updates_tx),
            crate::config::DEFAULT_MAX_OUTPUT_BYTES,
        )
        .await
    });

    let update = tokio::time::timeout(std::time::Duration::from_millis(500), updates_rx.recv())
        .await
        .expect("first output should stream promptly")
        .expect("stream should remain connected");
    assert_eq!(update.text, "streamed");
    assert!(!task.is_finished());

    let output = task.await.unwrap().unwrap();
    assert_eq!(output.stdout, "streamedfinished");
}

// Covers: a character split across two pipe reads must not stream as U+FFFD.
// Owner: pure unit (UTF-8 chunk boundary math)
#[test]
fn incomplete_utf8_suffix_len_holds_back_only_unfinished_sequences() {
    let cases: [(&[u8], usize); 8] = [
        (b"", 0),
        (b"abc", 0),
        // Complete two, three and four byte sequences hold back nothing.
        ("é".as_bytes(), 0),
        ("界".as_bytes(), 0),
        ("🦀".as_bytes(), 0),
        // Truncated sequences hold back exactly the bytes seen so far.
        (&"é".as_bytes()[..1], 1),
        (&"界".as_bytes()[..2], 2),
        (&"🦀".as_bytes()[..3], 3),
    ];

    for (bytes, expected) in cases {
        assert_eq!(incomplete_utf8_suffix_len(bytes), expected, "{bytes:?}");
    }
}

/// Yields one preset chunk per read so pipe boundaries are exact.
struct ChunkedReader {
    chunks: std::collections::VecDeque<Vec<u8>>,
}

impl tokio::io::AsyncRead for ChunkedReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Some(chunk) = self.chunks.pop_front() {
            buffer.put_slice(&chunk);
        }
        std::task::Poll::Ready(Ok(()))
    }
}

async fn collect_stream(chunks: &[&[u8]], max_output_bytes: usize) -> (String, Vec<String>) {
    let (updates_tx, mut updates_rx) = tokio::sync::mpsc::unbounded_channel();
    let reader = ChunkedReader {
        chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
    };
    let output = read_stream(
        reader,
        ShellStreamKind::Stdout,
        Some(updates_tx),
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        max_output_bytes,
    )
    .await
    .expect("in-memory reader cannot fail");
    let mut streamed = Vec::new();
    while let Ok(update) = updates_rx.try_recv() {
        streamed.push(update.text);
    }
    (output, streamed)
}

// Covers: streamed shell output must be decoded on character boundaries, so the
// live card never shows replacement characters the finished card lacks.
// Owner: pure unit (stream decoding)
#[tokio::test]
async fn streams_characters_split_across_reads_without_replacement() {
    let bytes = "a界b".as_bytes();
    let (output, streamed) = collect_stream(&[&bytes[..2], &bytes[2..]], 1024).await;

    assert_eq!(output, "a界b");
    assert_eq!(streamed, vec!["a".to_string(), "界b".to_string()]);
}

// Covers: a command that never stops printing must not grow the buffer without
// bound; output stops at the configured cap and says so.
// Owner: pure unit (stream capping)
#[tokio::test]
async fn caps_streamed_output_at_the_configured_limit() {
    let (output, streamed) = collect_stream(&[b"abcd", b"efgh"], 6).await;

    assert_eq!(output, format!("abcdef{TRUNCATION_NOTICE}"));
    assert_eq!(streamed.concat(), format!("abcdef{TRUNCATION_NOTICE}"));
}

#[test]
fn display_text_preserves_output_and_context_state() {
    let output = ShellOutput {
        shell: "bash".into(),
        command: "printf hello".into(),
        stdout: "hello".into(),
        stderr: String::new(),
        exit_code: "0".into(),
        ok: true,
    };

    assert_eq!(
        display_text(&output, /*included_in_context*/ true),
        "✓ $ printf hello\nhello"
    );
}
