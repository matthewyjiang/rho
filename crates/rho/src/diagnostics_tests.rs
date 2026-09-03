use pretty_assertions::assert_eq;

use super::*;

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
    let diagnostics = RuntimeDiagnostics::new(&config);

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

    diagnostics.record_context(ContextUsage::estimated(123, Some(1_000)));
    let response: serde_json::Value =
        serde_json::from_str(&diagnostics.response("context").unwrap()).unwrap();
    assert_eq!(response["tokens"], 123);
    assert_eq!(response["context_window"], 1_000);
    assert_eq!(response["source"], "Estimated");

    diagnostics.update_identity("anthropic", "claude-test", ReasoningLevel::Low);
    assert_eq!(diagnostics.response("context").unwrap(), "null");
}

#[test]
fn rejects_unknown_actions_with_valid_choices() {
    let diagnostics = test_diagnostics("openai", "gpt-test");

    let error = diagnostics.response("everything").unwrap_err();

    assert_eq!(
        error,
        "unknown rho diagnostics action 'everything'; expected one of: info, context, prompt_sources, tools, hooks, config"
    );
}

#[test]
fn runtime_updates_do_not_replace_restart_only_config() {
    let config = Config {
        max_output_bytes: 4_096,
        max_tool_output_lines: 10,
        ..Config::default()
    };
    let diagnostics = RuntimeDiagnostics::new(&config);
    diagnostics.update_max_tool_output_lines(25);
    diagnostics.update_check_for_updates(false);
    diagnostics.update_edit_tool("str_replace");
    diagnostics.update_compaction_config(&CompactionConfig {
        auto_compact: true,
        threshold_percent: 70,
        target_percent: 40,
    });

    let response: serde_json::Value =
        serde_json::from_str(&diagnostics.response("config").unwrap()).unwrap();

    assert_eq!(
        response,
        serde_json::json!({
            "max_output_bytes": 4_096,
            "max_tool_output_lines": 25,
            "auto_compact": true,
            "compact_threshold_percent": 70,
            "compact_target_percent": 40,
            "web_search_hosted": true,
            "web_search_provider": "auto",
            "xai_image_generation": true,
            "check_for_updates": false,
            "enable_subagents": true,
            "agent_concurrency": 10,
            "advisor_mode": false,
            "edit_tool": "str_replace",
            "rtk": true,
            "source": "live values used by this process; restart-only settings may differ from saved config"
        })
    );
}
