use super::*;

#[tokio::test]
async fn rejects_unsupported_action() {
    let tool = Rho::new(crate::diagnostics::test_diagnostics("openai", "gpt-test"));

    let error = tool
        .call(
            serde_json::json!({"action": "mutate"}),
            ToolContext {
                cwd: std::env::current_dir().unwrap(),
                max_output_bytes: 12_000,
            },
            "call-1".into(),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("unknown rho diagnostics action"));
}
