use pretty_assertions::assert_eq;

use super::super::{Config, ConfigWarning};

fn parse_settings(text: &str) -> anyhow::Result<(Config, Vec<ConfigWarning>)> {
    Config::parse_settings_with_warnings(text)
}

fn load_error(text: &str) -> String {
    format!("{:#}", parse_settings(text).unwrap_err())
}

// Covers: a typed edit rewrites only the keys it changed; comments, key
// order, and untouched keys stay as the user wrote them.
// Owner: config save
#[test]
fn writing_a_key_keeps_comments_and_key_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = r#"# conversation defaults
[model]
provider = "openai" # pinned
model = "gpt-5.5"
auth = "api-key"

[compaction]
# when to compact
compact_threshold_percent = 80 # tuned by hand
auto_compact = true
"#;
    std::fs::write(&path, original).unwrap();

    Config::edit(&path, |config| {
        config.compact_threshold_percent = 70;
        config.fast_mode = true;
    })
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"# conversation defaults
[model]
provider = "openai" # pinned
model = "gpt-5.5"
auth = "api-key"
fast_mode = true

[compaction]
# when to compact
compact_threshold_percent = 70 # tuned by hand
auto_compact = true
"#
    );
}

// Covers: a scalar edit must not re-encode hand-edited structured tables.
// Owner: config save
#[test]
fn writing_a_scalar_leaves_providers_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = r#"[behavior]
rtk = true

[providers.custom.composer]   # local gateway
base_url   =   "http://127.0.0.1:8787/v1"   # spacing kept
"#;
    std::fs::write(&path, original).unwrap();

    Config::edit(&path, |config| config.rtk = false).unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original.replace("rtk = true", "rtk = false")
    );
}

// Covers: out-of-range values fail loudly instead of being clamped, and the
// error names the key, the limit, and the asked value.
// Owner: config load
#[test]
fn out_of_range_values_name_the_key_limit_and_value() {
    for (text, expected) in [
        (
            "[compaction]\ncompact_threshold_percent = 101\n",
            "`compaction.compact_threshold_percent`: must be at most 100; got 101",
        ),
        (
            "[compaction]\ncompact_target_percent = 0\n",
            "`compaction.compact_target_percent`: must be at least 1; got 0",
        ),
        (
            "[behavior]\nagent_concurrency = 65\n",
            "`behavior.agent_concurrency`: must be at most 64; got 65",
        ),
        (
            "[output]\nmax_output_bytes = -1\n",
            "`output.max_output_bytes`: must be at least 1; got -1",
        ),
        (
            "[questionnaire]\ntimeout_seconds = 0\n",
            "`questionnaire.timeout_seconds`: must be at least 1; got 0",
        ),
    ] {
        assert_eq!(load_error(text), expected, "{text}");
    }
}

// Covers: misspelled keys and 2.x spellings fail the load instead of being
// silently ignored.
// Owner: config load
#[test]
fn unknown_and_2x_keys_are_rejected() {
    for (text, expected) in [
        ("provder = \"openai\"\n", "unknown key `provder`"),
        ("provider = \"openai\"\n", "unknown key `provider`"),
        ("title_model = \"gpt-5.5\"\n", "unknown key `title_model`"),
        (
            "[display]\nzen_mode = true\n",
            "unknown key `display.zen_mode`",
        ),
        (
            "[display]\ntheme = \"dark\"\n",
            "unknown key `display.theme`",
        ),
        (
            "[behavior]\ncheck_for_updates = false\n",
            "unknown key `behavior.check_for_updates`",
        ),
        (
            "[web_search]\nhosted = true\n",
            "unknown key `web_search.hosted`",
        ),
        (
            "[keybindings]\nopen_editor = \"ctrl+e\"\n",
            "unknown key `keybindings.open_editor`",
        ),
        (
            "model = \"gpt-5.5\"\n",
            "`model`: must be a table with provider and model keys",
        ),
        (
            "[behavior]\nedit_tool = \"hashline\"\n",
            "`behavior.edit_tool`: must be one of auto, str_replace, apply_patch; got \"hashline\"",
        ),
    ] {
        assert_eq!(load_error(text), expected, "{text}");
    }
}

// Covers: every bad key in one file is reported at once, not one per load.
// Owner: config load
#[test]
fn every_bad_key_is_reported() {
    assert_eq!(
        load_error("zen_mode = true\n[compaction]\ncompact_threshold_percent = 0\n"),
        "unknown key `zen_mode`\n`compaction.compact_threshold_percent`: must be at least 1; got 0"
    );
}

// Covers: the compaction target must stay below the threshold, at load and
// when an edit would break it (and then nothing is written).
// Owner: config load
#[test]
fn compaction_target_at_or_above_threshold_is_rejected() {
    assert_eq!(
        load_error(
            "[compaction]\ncompact_threshold_percent = 60\ncompact_target_percent = 60\n"
        ),
        "`compaction.compact_target_percent`: must be below compaction.compact_threshold_percent (60); got 60"
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "[compaction]\ncompact_threshold_percent = 60\ncompact_target_percent = 40\n";
    std::fs::write(&path, original).unwrap();
    assert!(Config::edit(&path, |config| config.compact_target_percent = 70).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

// Covers: every registry row saves to the key it loads from, so a config
// saved from scratch reloads to the same values.
// Owner: config registry
#[test]
fn every_agent_setting_round_trips_through_a_fresh_file() {
    let (loaded, _) = parse_settings(
        r#"
[model]
provider = "anthropic"
model = "claude-sonnet-4-5"
auth = "anthropic-api-key"
reasoning = "high"
fast_mode = true

[behavior]
permission_mode = "auto"
advisor_mode = true
enable_subagents = false
agent_concurrency = 3
workspace_rewind = false
keep_cache_warm = true
inline_shell = "zsh"
edit_tool = "apply_patch"
rtk = false
credential_store = "file"

[internal_agents.advisor]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
reasoning = "low"

[internal_agents.permission-classifier]
provider = "openai"
model = "gpt-5.5-mini"
auth = "api-key"
reasoning = "minimal"

[questionnaire]
timeout_seconds = 30

[compaction]
auto_compact = false
compact_threshold_percent = 70
compact_target_percent = 30

[output]
max_output_bytes = 4096

[codemode]
mode = "only"

[web_search]
mode = "backend"
backend = "exa"
openai.connection = "codex"
openai.api_base_url = "https://openai.example/v1"
exa.connection = "mcp"
exa.api_base_url = "https://exa.example"
exa.mcp_url = "https://exa.example/mcp"
brave.api_base_url = "https://brave.example"
firecrawl.api_base_url = "https://firecrawl.example"

[xai]
image_generation = true
"#,
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    loaded.save(Some(path.clone())).unwrap();

    assert_eq!(
        format!("{:#?}", Config::load(Some(path)).unwrap()),
        format!("{loaded:#?}")
    );
}

// Covers: reading a missing file is the defaults and never creates it.
// Owner: config load
#[test]
fn loading_a_missing_file_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::load(Some(path.clone())).unwrap();
    assert!(!path.exists());
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
[internal_agents.compaction]
runtime = "claude-cli"
model = "opus"
"#,
    )
    .unwrap();

    assert_eq!(
        conversation_triple(&config, "compaction"),
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
            to: "\"rho\" with the conversation selection; agent 'compaction' cannot delegate"
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
[model]
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
fn conversation_triple(config: &Config, id: &str) -> (String, String, String) {
    let selection = config.internal_agent_model(id).unwrap().expect_rho();
    (
        selection.provider.clone(),
        selection.model.clone(),
        selection.auth.clone(),
    )
}

// Covers: a catalog (plugin) agent's entry replaces the model its file names,
// so an entry missing provider or model (a bare-model alias names no
// provider; a provider-qualified one names both), or naming another runtime,
// is dropped with a warning instead of silently pinning the conversation model;
// internal agents keep filling the gap from the conversation model.
// Owner: config load
#[test]
fn catalog_agent_entries_without_a_model_are_dropped_with_a_warning() {
    let (config, warnings) = parse_settings(
        r#"
[internal_agents.plugin-reviewer]
reasoning = "high"

[internal_agents.plugin-auditor]
runtime = "claude-cli"
model = "opus"

[internal_agents.plugin-fast]
model = "@fast"

[internal_agents.plugin-deep]
model = "@deep"

[model.aliases]
deep = "anthropic/claude-opus-4-8"
fast = "gpt-5.5-mini"

[internal_agents.compaction]
reasoning = "high"
"#,
    )
    .unwrap();

    assert_eq!(
        (
            config.internal_agent_model("plugin-reviewer"),
            config.internal_agent_model("plugin-auditor"),
            config.internal_agent_model("plugin-fast"),
            config
                .internal_agent_model("plugin-deep")
                .and_then(crate::config::InternalAgentModelConfig::rho)
                .map(|rho| (rho.provider.as_str(), rho.model.as_str())),
            conversation_triple(&config, "compaction"),
        ),
        (
            None,
            None,
            None,
            Some(("anthropic", "claude-opus-4-8")),
            (
                config.provider.clone(),
                config.model.clone(),
                config.auth.clone()
            )
        )
    );
    assert_eq!(
        warnings,
        ["plugin-auditor", "plugin-fast", "plugin-reviewer"]
            .map(|id| ConfigWarning::Normalized {
                key: "internal_agents",
                from: format!("for agent '{id}'"),
                to: "no override; a catalog agent's entry must name a Rho provider and model"
                    .into(),
            })
            .to_vec()
    );
}

// Covers: a custom host configured before Rho shipped a built-in of the same
// name (OpenCode Zen as `opencode`) loads with a warning when it points at the
// built-in's API base, and fails loudly for any other base
// Owner: config load
#[test]
fn custom_provider_shadowed_by_a_builtin_is_skipped_with_a_warning() {
    let (config, warnings) = parse_settings(
        r#"
[providers.custom.opencode]
base_url = "https://opencode.ai/zen/v1/"

[providers.custom.composer]
base_url = "http://127.0.0.1:8787/v1"
"#,
    )
    .unwrap();

    assert_eq!(
        config.providers.custom.keys().collect::<Vec<_>>(),
        ["composer"]
    );
    assert_eq!(
        warnings,
        vec![ConfigWarning::ShadowedCustomProvider {
            name: "opencode".into()
        }]
    );

    // Another gateway must not hand its stored key to opencode.ai, and the Go
    // route on the same host must not silently become Zen billing.
    for base_url in [
        "https://private-gateway.example/v1",
        "https://opencode.ai/inference/go/v1",
    ] {
        assert!(
            parse_settings(&format!(
                "[providers.custom.opencode]\nbase_url = \"{base_url}\"\n"
            ))
            .is_err(),
            "{base_url}"
        );
    }
}
