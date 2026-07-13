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
        std::time::Duration::from_secs(4),
        Bash::new(false).call(
            json!({"command": "sleep 30 & printf done"}),
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
