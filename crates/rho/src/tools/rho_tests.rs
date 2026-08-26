use super::*;

#[tokio::test]
async fn advertises_valid_actions_and_rejects_unsupported_ones() {
    let tool = Rho::new(crate::diagnostics::test_diagnostics("openai", "gpt-test"));
    let spec = tool.spec();
    assert_eq!(
        spec.input_schema["properties"]["action"]["enum"],
        serde_json::json!([
            "info",
            "context",
            "prompt_sources",
            "tools",
            "hooks",
            "config"
        ])
    );
    assert_eq!(super::super::canonical_tool_is_mutating("rho"), Some(false));

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
