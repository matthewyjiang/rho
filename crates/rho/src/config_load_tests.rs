use super::{parse_settings, ConfigWarning};
use pretty_assertions::assert_eq;

// Covers: unknown top-level config keys are a hard load error
// Owner: config load
#[test]
fn unknown_top_level_keys_are_rejected() {
    let error = parse_settings("provder = \"openai\"\nmodel = \"gpt-5.5\"\n").unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("unknown field `provder`"),
        "unexpected error: {message}"
    );
}

// Covers: unknown nested config keys are a hard load error
// Owner: config load
#[test]
fn unknown_nested_keys_are_rejected() {
    let error = parse_settings(
        r#"
[display]
max_tool_output_liness = 3
"#,
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("unknown field `max_tool_output_liness`"),
        "unexpected error: {message}"
    );
}

// Covers: unknown providers / ollama / internal_agent fields are hard errors
// Owner: config load
#[test]
fn unknown_provider_and_internal_agent_keys_are_rejected() {
    for (toml, field) in [
        (
            r#"
[providers]
unknown = {}
"#,
            "unknown",
        ),
        (
            r#"
[providers.ollama]
bad_key = "x"
"#,
            "bad_key",
        ),
        (
            r#"
[internal_agents.reviewer]
bad_key = "x"
"#,
            "bad_key",
        ),
    ] {
        let error = parse_settings(toml).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(&format!("unknown field `{field}`")),
            "expected unknown field `{field}` in: {message}"
        );
    }
}

// Covers: max_tool_output_lines below 1 clamps with a warning
// Owner: config load
#[test]
fn max_tool_output_lines_clamp_warns() {
    let (config, warnings) = parse_settings(
        r#"
[display]
max_tool_output_lines = 0
"#,
    )
    .unwrap();

    assert_eq!(config.max_tool_output_lines, 1);
    assert_eq!(
        warnings,
        vec![ConfigWarning::Clamped {
            key: "display.max_tool_output_lines",
            from: "0".into(),
            to: "1".into(),
        }]
    );
}

// Covers: unsupported web_search.provider normalizes to auto with a warning
// Owner: config load
#[test]
fn unsupported_web_search_provider_normalizes_with_warning() {
    let (config, warnings) = parse_settings(
        r#"
[web_search]
provider = "unknown"
"#,
    )
    .unwrap();

    assert_eq!(
        config.web_search_provider,
        super::super::SearchProvider::Auto
    );
    assert_eq!(
        warnings,
        vec![ConfigWarning::Normalized {
            key: "web_search.provider",
            from: "\"unknown\"".into(),
            to: "\"auto\"".into(),
        }]
    );
}

// Covers: each configured edit preference selects its distinct model-facing tool
// Owner: config load
#[test]
fn edit_tool_preferences_load_from_behavior_config() {
    for (value, expected) in [
        ("auto", super::super::EditTool::Auto),
        (
            "hashline",
            super::super::EditTool::Pinned(rho_tools::EditFormat::Hashline),
        ),
        (
            "apply_patch",
            super::super::EditTool::Pinned(rho_tools::EditFormat::ApplyPatch),
        ),
        (
            "str_replace",
            super::super::EditTool::Pinned(rho_tools::EditFormat::StrReplace),
        ),
    ] {
        let (config, warnings) =
            parse_settings(&format!("[behavior]\nedit_tool = {value:?}\n")).unwrap();

        assert_eq!(config.edit_tool, expected);
        assert_eq!(warnings, Vec::<ConfigWarning>::new());
    }
}

// Covers: known keys alone produce no warnings
// Owner: config load
#[test]
fn known_config_produces_no_warnings() {
    let (_config, warnings) = parse_settings(
        r#"
[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
"#,
    )
    .unwrap();
    assert_eq!(warnings, Vec::<ConfigWarning>::new());
}

// Covers: legacy top-level keys remain accepted under deny_unknown_fields
// Owner: config load
#[test]
fn legacy_top_level_keys_still_load() {
    let (config, warnings) = parse_settings(
        r#"
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
max_tool_output_lines = 4
"#,
    )
    .unwrap();
    assert_eq!(config.provider, "openai");
    assert_eq!(config.model, "gpt-5.5");
    assert_eq!(config.max_tool_output_lines, 4);
    assert_eq!(warnings, Vec::<ConfigWarning>::new());
}

// Covers: display.theme loads and defaults to terminal
// Owner: config load
#[test]
fn theme_loads_from_display_group() {
    let (config, warnings) = parse_settings(
        r#"
[display]
theme = "one-half-dark"
"#,
    )
    .unwrap();
    assert_eq!(config.theme, "one-half-dark");
    assert_eq!(warnings, Vec::<ConfigWarning>::new());

    let (padded, padded_warnings) = parse_settings(
        r#"
[display]
theme = "  one-half-light  "
"#,
    )
    .unwrap();
    assert_eq!(padded.theme, "one-half-light");
    assert_eq!(padded_warnings, Vec::<ConfigWarning>::new());

    let (blank, blank_warnings) = parse_settings(
        r#"
[display]
theme = "   "
"#,
    )
    .unwrap();
    assert_eq!(blank.theme, "terminal");
    assert_eq!(blank_warnings, Vec::<ConfigWarning>::new());

    let (defaulted, default_warnings) = parse_settings("").unwrap();
    assert_eq!(defaulted.theme, "terminal");
    assert_eq!(default_warnings, Vec::<ConfigWarning>::new());
}

// Covers: display.zen_mode loads and defaults off
// Owner: config load
#[test]
fn zen_mode_loads_from_display_group() {
    let (config, warnings) = parse_settings(
        r#"
[display]
zen_mode = true
"#,
    )
    .unwrap();
    assert!(config.zen_mode);
    assert_eq!(warnings, Vec::<ConfigWarning>::new());

    let (defaulted, default_warnings) = parse_settings(
        r#"
[display]
show_reasoning_output = true
"#,
    )
    .unwrap();
    assert!(!defaulted.zen_mode);
    assert_eq!(default_warnings, Vec::<ConfigWarning>::new());
}

// Covers: top-level zen_mode folds into [display]; grouped value wins
// Owner: config load
#[test]
fn legacy_top_level_zen_mode_loads() {
    let (config, warnings) = parse_settings(
        r#"
zen_mode = true
"#,
    )
    .unwrap();
    assert!(config.zen_mode);
    assert_eq!(warnings, Vec::<ConfigWarning>::new());

    let (grouped_wins, grouped_warnings) = parse_settings(
        r#"
zen_mode = true

[display]
zen_mode = false
"#,
    )
    .unwrap();
    assert!(!grouped_wins.zen_mode);
    assert_eq!(grouped_warnings, Vec::<ConfigWarning>::new());
}

// Covers: [model] group provider wins over a top-level provider
// Owner: config load
#[test]
fn model_group_provider_overrides_top_level_provider() {
    let (config, warnings) = parse_settings(
        r#"
provider = "openai"
auth = "api-key"
[model]
provider = "anthropic"
model = "claude-sonnet-4-5"
auth = "anthropic-api-key"
"#,
    )
    .unwrap();
    assert_eq!(config.provider, "anthropic");
    assert_eq!(config.model, "claude-sonnet-4-5");
    assert_eq!(config.auth, "anthropic-api-key");
    assert_eq!(warnings, Vec::<ConfigWarning>::new());
}

// Covers: empty [title] is a no-op; legacy title_* keys still apply
// Owner: config load
#[test]
fn empty_title_section_keeps_legacy_title_keys() {
    let (config, warnings) = parse_settings(
        r#"
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
title_provider = "anthropic"
title_model = "claude-sonnet-4-5"
title_auth = "anthropic-api-key"
[title]
"#,
    )
    .unwrap();
    let title = config
        .internal_agents
        .get("session-title")
        .expect("session-title internal agent");
    assert_eq!(title.expect_rho().provider, "anthropic");
    assert_eq!(title.expect_rho().model, "claude-sonnet-4-5");
    assert_eq!(title.expect_rho().auth, "anthropic-api-key");
    assert_eq!(warnings, Vec::<ConfigWarning>::new());
}

// Covers: behavior.advisor_mode loads and defaults off
// Owner: config load
#[test]
fn advisor_mode_loads_from_behavior_group() {
    let (config, warnings) = parse_settings(
        r#"
[behavior]
advisor_mode = true
"#,
    )
    .unwrap();
    assert!(config.advisor_mode);
    assert_eq!(warnings, Vec::<ConfigWarning>::new());

    let (defaulted, default_warnings) = parse_settings(
        r#"
[behavior]
enable_subagents = true
"#,
    )
    .unwrap();
    assert!(!defaulted.advisor_mode);
    assert_eq!(default_warnings, Vec::<ConfigWarning>::new());
}

// Covers: Rho-only keys under a delegating entry are reported and dropped, not
// silently kept where nothing would ever read them.
// Owner: config load
#[test]
fn a_claude_cli_internal_agent_drops_rho_provider_and_auth() {
    let (config, warnings) = parse_settings(
        r#"
[internal_agents.advisor]
runtime = "claude-cli"
model = "opus"
provider = "anthropic"
auth = "anthropic-api-key"
"#,
    )
    .unwrap();

    assert_eq!(
        config
            .internal_agent_model(crate::agent::ADVISOR_AGENT_ID)
            .map(|selection| selection.target.clone()),
        Some(crate::config::InternalAgentTarget::ClaudeCli {
            model: Some("opus".into())
        })
    );
    assert_eq!(
        warnings,
        vec![
            ConfigWarning::Normalized {
                key: "internal_agents.provider",
                from: "\"anthropic\"".into(),
                to: "no value; [internal_agents.advisor] runtime claude-cli has no Rho provider or auth".into(),
            },
            ConfigWarning::Normalized {
                key: "internal_agents.auth",
                from: "\"anthropic-api-key\"".into(),
                to: "no value; [internal_agents.advisor] runtime claude-cli has no Rho provider or auth".into(),
            },
        ]
    );
}

// Covers: only agents declared as delegating may delegate, so a hand-edited
// runtime key on a structured internal agent falls back to the conversation
// selection rather than keeping the rejected runtime's model name.
// Owner: config load
#[test]
fn internal_agents_that_cannot_delegate_fall_back_to_the_rho_runtime() {
    let (config, warnings) = parse_settings(
        r#"
[internal_agents.goal-judge]
runtime = "claude-cli"
model = "opus"
"#,
    )
    .unwrap();

    assert_eq!(
        conversation_triple(&config, "goal-judge"),
        (
            config.provider.clone(),
            config.model.clone(),
            config.auth.clone()
        )
    );
    assert_eq!(
        warnings,
        vec![ConfigWarning::Normalized {
            key: "internal_agents.runtime",
            from: "\"claude-cli\"".into(),
            to:
                "\"rho\" with the conversation selection; internal agent 'goal-judge' cannot delegate"
                    .into(),
        }]
    );
}

// Covers: an unusable runtime value must not fail the whole config load, and it
// takes the entry's provider, model, and auth with it. Keeping only some of
// them would route a conversation model through the entry's credentials.
// Owner: config load
#[test]
fn an_unknown_internal_agent_runtime_falls_back_with_a_warning() {
    let (config, warnings) = parse_settings(
        r#"
provider = "anthropic"
model = "claude-sonnet-4-5"
auth = "anthropic-api-key"

[internal_agents.advisor]
runtime = "codex"
model = "codex-mini"
provider = "openai"
auth = "api-key"
"#,
    )
    .unwrap();

    assert_eq!(
        conversation_triple(&config, crate::agent::ADVISOR_AGENT_ID),
        (
            "anthropic".to_string(),
            "claude-sonnet-4-5".to_string(),
            "anthropic-api-key".to_string()
        )
    );
    assert_eq!(
        warnings,
        vec![ConfigWarning::Normalized {
            key: "internal_agents.runtime",
            from: "\"codex\"".into(),
            to: "\"rho\" with the conversation selection".into(),
        }]
    );
}

/// The stored Rho provider, model, and auth for one internal agent.
fn conversation_triple(config: &super::Config, id: &str) -> (String, String, String) {
    let selection = config.internal_agent_model(id).unwrap().expect_rho();
    (
        selection.provider.clone(),
        selection.model.clone(),
        selection.auth.clone(),
    )
}

// Covers: Auto preference resolves EditTool and composes display labels for
// the active provider.
// Owner: config EditTool preference
#[test]
fn auto_edit_tool_resolves_preferred_format_for_provider() {
    use super::EditTool;

    assert_eq!(
        EditTool::Auto.resolve("openai-codex"),
        rho_tools::EditFormat::ApplyPatch
    );
    assert_eq!(
        EditTool::Auto.display_label("anthropic"),
        "auto (str_replace)"
    );
}
