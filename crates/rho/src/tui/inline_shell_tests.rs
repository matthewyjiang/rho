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
        "$ printf hello\n\nhello"
    );
}

#[test]
fn inline_powershell_uses_ps_prompt_and_hides_diagnostics() {
    let output = ShellOutput {
        shell: "powershell".into(),
        command: "Write-Output hello".into(),
        stdout: "hello\n".into(),
        stderr: "warning\n".into(),
        exit_code: "1".into(),
        ok: false,
    };

    assert_eq!(
        display_text(&output, /*included_in_context*/ false),
        "PS Write-Output hello\n\nhello"
    );
}
