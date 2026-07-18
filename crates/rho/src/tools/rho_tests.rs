use pretty_assertions::assert_eq;

use super::*;

#[tokio::test]
async fn info_returns_live_runtime_identity() {
    let diagnostics = crate::diagnostics::test_diagnostics("openai", "gpt-test");
    let tool = Rho::new(diagnostics.clone());
    diagnostics.update_identity(
        "openai-codex",
        "gpt-current",
        rho_providers::reasoning::ReasoningLevel::High,
    );

    let result = tool
        .call(
            serde_json::json!({"action": "info"}),
            ToolContext {
                cwd: std::env::current_dir().unwrap(),
                max_output_bytes: 12_000,
            },
            "call-1".into(),
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&result.content).unwrap();

    assert_eq!(value["provider"], "openai-codex");
    assert_eq!(value["model"], "gpt-current");
    assert_eq!(value["reasoning"], "high");
}

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
