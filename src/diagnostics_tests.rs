use pretty_assertions::assert_eq;

use super::*;

#[test]
fn info_returns_only_runtime_identity() {
    let diagnostics = test_diagnostics("openai", "gpt-test");

    let response = diagnostics.response("info").unwrap();
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "rho_version": env!("CARGO_PKG_VERSION"),
            "provider": "openai",
            "model": "gpt-test",
            "reasoning": "medium"
        })
    );
}

#[test]
fn config_excludes_credentials_and_user_content() {
    let config = Config {
        auth: "secret-auth-mode".into(),
        favorite_models: vec!["private/favorite".into()],
        prompt_templates: [("private".into(), "secret template".into())]
            .into_iter()
            .collect(),
        ..Config::default()
    };
    let diagnostics = RuntimeDiagnostics::new(&config, Vec::new(), Vec::new());

    let response = diagnostics.response("config").unwrap();

    assert!(!response.contains("secret-auth-mode"));
    assert!(!response.contains("private/favorite"));
    assert!(!response.contains("secret template"));
    assert!(response.contains("max_output_bytes"));
}

#[test]
fn context_is_null_until_usage_is_observed() {
    let diagnostics = test_diagnostics("openai", "gpt-test");

    assert_eq!(diagnostics.response("context").unwrap(), "null");

    diagnostics.update_context(ContextUsage::estimated(123, Some(1_000)));
    let response: serde_json::Value =
        serde_json::from_str(&diagnostics.response("context").unwrap()).unwrap();
    assert_eq!(response["tokens"], 123);
    assert_eq!(response["context_window"], 1_000);
    assert_eq!(response["source"], "Estimated");
}

#[test]
fn rejects_unknown_actions_with_skill_guidance() {
    let diagnostics = test_diagnostics("openai", "gpt-test");

    let error = diagnostics.response("everything").unwrap_err();

    assert!(error.contains("unknown rho diagnostics action 'everything'"));
    assert!(error.contains("rho-diagnostics skill"));
}
