use serde_json::json;

use super::*;

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
