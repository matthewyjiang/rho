use pretty_assertions::assert_eq;

use super::{Config, EffectiveModelSource};

// Covers: invalid config errors must name the config file path
// Owner: config load
#[test]
fn invalid_config_errors_include_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken-config.toml");
    std::fs::write(&path, "permission_mode = [\n").unwrap();

    let error = Config::load(Some(path.clone())).unwrap_err();
    let message = format!("{error:#}");

    assert!(
        message.contains(&path.display().to_string()),
        "error must name the config file path: {message}"
    );
}

// Covers: auto compaction defaults on without a [compaction] section, and an
// explicit opt-out survives load and a save round-trip.
// Owner: config load/save
#[test]
fn auto_compact_defaults_on_and_explicit_opt_out_round_trips() {
    for (contents, expected) in [("", true), ("[compaction]\nauto_compact = false\n", false)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();

        let config = Config::load(Some(path.clone())).unwrap();
        config.save(Some(path.clone())).unwrap();
        let reloaded = Config::load(Some(path)).unwrap();

        let threshold = crate::compaction::CompactionConfig::from(&reloaded)
            .threshold_tokens(/*context_window*/ 200_000);
        assert_eq!(threshold.is_some(), expected, "config: {contents:?}");
    }
}

fn alias_config(path: &std::path::Path, model: &str) {
    std::fs::write(
        path,
        format!(
            r#"
[model]
provider = "openai"
model = "{model}"

[model.aliases]
deep = "anthropic/claude-opus-4-8"
fast = "gpt-5.5-mini"
"#
        ),
    )
    .unwrap();
}

#[test]
fn model_alias_resolves_session_model_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@deep");

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.provider, "anthropic");
    assert_eq!(config.model, "claude-opus-4-8");
    assert_eq!(config.current_model_alias(), Some("deep"));
}

#[test]
fn bare_model_alias_keeps_configured_provider_and_auth() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@fast");

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.provider, "openai");
    assert_eq!(config.model, "gpt-5.5-mini");
    assert_eq!(config.auth, Config::default().auth);
    assert_eq!(config.current_model_alias(), Some("fast"));
}

#[test]
fn concrete_model_id_wins_over_identically_named_alias() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"

[model.aliases]
"gpt-5.5" = "anthropic/claude-opus-4-8"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.provider, "openai");
    assert_eq!(config.model, "gpt-5.5");
    assert_eq!(config.current_model_alias(), None);
}

// Covers: an alias to an unknown provider fails load whether or not it is used.
// Owner: config load
#[test]
fn model_alias_targeting_unknown_provider_is_a_config_error() {
    for (case, model, alias) in [("used", "@deep", "deep"), ("unused", "gpt-5.5", "unused")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!(
                "[model]\nmodel = \"{model}\"\n\n[model.aliases]\n{alias} = \"nonexistent/model-x\"\n"
            ),
        )
        .unwrap();

        let error = Config::load(Some(path)).unwrap_err();

        assert!(
            format!("{error:#}").contains(&format!(
                "model alias '{alias}' targets unknown provider 'nonexistent'"
            )),
            "{case}: {error:#}"
        );
    }
}

// Covers: an undefined alias fails load and names where it was referenced.
// Owner: config load
#[test]
fn undefined_model_alias_names_reference_site() {
    for (config, site) in [
        ("[model]\nmodel = \"@missing\"\n", "session model"),
        (
            "[model]\nmodel = \"gpt-5.5\"\n\n[internal_agents.session-title]\nmodel = \"@missing\"\n",
            "internal agent 'session-title' model",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, config).unwrap();

        let error = Config::load(Some(path))
        .unwrap_err();

        assert!(
            format!("{error:#}").contains(&format!(
                "{site}: model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
            )),
            "{error:#}"
        );
    }
}

#[test]
fn title_model_alias_resolves_provider_model_and_auth_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"

[model.aliases]
titler = "anthropic/claude-haiku-4-5"

[internal_agents.session-title]
provider = "anthropic"
model = "@titler"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    let selection = config.internal_agent_model("session-title").unwrap();
    assert_eq!(selection.expect_rho().provider, "anthropic");
    assert_eq!(selection.expect_rho().model, "claude-haiku-4-5");
    assert_eq!(selection.expect_rho().auth, "anthropic-api-key");
    assert_eq!(
        config.current_internal_agent_model_alias("session-title"),
        Some("titler")
    );
}

#[test]
fn stale_title_model_alias_saves_the_concrete_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "gpt-5.5");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[internal_agents.session-title]\nmodel = \"@deep\"\n");
    std::fs::write(&path, text).unwrap();

    let mut config = Config::load(Some(path.clone())).unwrap();
    config
        .internal_agents
        .get_mut("session-title")
        .unwrap()
        .expect_rho_mut()
        .model = "claude-sonnet-4-5".into();
    assert_eq!(
        config.current_internal_agent_model_alias("session-title"),
        None
    );
    config.save(Some(path.clone())).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("model = \"claude-sonnet-4-5\""), "{saved}");
    let reloaded = Config::load(Some(path)).unwrap();
    assert_eq!(
        reloaded
            .internal_agent_model("session-title")
            .unwrap()
            .expect_rho()
            .model,
        "claude-sonnet-4-5"
    );
    assert_eq!(
        reloaded.current_internal_agent_model_alias("session-title"),
        None
    );
}

#[test]
fn effective_internal_agent_models_are_independent() {
    let mut config = Config::default();
    config.set_internal_agent_model(
        "session-title",
        "anthropic".into(),
        "claude-haiku-4-5".into(),
        "anthropic-api-key".into(),
    );

    let title = config.effective_internal_agent_model("session-title");
    let judge = config.effective_internal_agent_model("compaction");

    assert_eq!(title.provider, "anthropic");
    assert_eq!(title.model, "claude-haiku-4-5");
    assert_eq!(title.source, EffectiveModelSource::Override);
    assert_eq!(judge.provider, config.provider);
    assert_eq!(judge.model, config.model);
    assert_eq!(judge.auth, config.auth);
    assert_eq!(judge.source, EffectiveModelSource::Conversation);
}

#[test]
fn model_alias_round_trips_through_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@deep");

    let config = Config::load(Some(path.clone())).unwrap();
    config.save(Some(path.clone())).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("model = \"@deep\""), "{saved}");
    assert!(saved.contains("[model.aliases]"), "{saved}");
    let reloaded = Config::load(Some(path)).unwrap();
    assert_eq!(reloaded.provider, "anthropic");
    assert_eq!(reloaded.model, "claude-opus-4-8");
    assert_eq!(reloaded.current_model_alias(), Some("deep"));
}

#[test]
fn stale_model_alias_saves_the_concrete_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@deep");

    let mut config = Config::load(Some(path.clone())).unwrap();
    config.model = "claude-haiku-4-5".into();
    assert_eq!(config.current_model_alias(), None);
    config.save(Some(path.clone())).unwrap();

    let reloaded = Config::load(Some(path)).unwrap();
    assert_eq!(reloaded.model, "claude-haiku-4-5");
    assert_eq!(reloaded.current_model_alias(), None);
}

#[test]
fn provider_base_url_rejects_invalid_or_unsupported_urls() {
    for (section, field) in [
        ("[providers.ollama]", "providers.ollama.base_url"),
        (
            "[providers.custom.composer]",
            "providers.custom.composer.base_url",
        ),
    ] {
        for base_url in [
            "not a URL",
            "file:///tmp/models",
            "http://user:secret@localhost:11434/v1",
            "http://localhost:11434/v1?token=secret",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, format!("{section}\nbase_url = {base_url:?}\n")).unwrap();

            let error = Config::load(Some(path)).unwrap_err();

            assert!(format!("{error:#}").contains(field), "{error:#}");
        }
    }
}

#[test]
fn keyless_provider_inference_sets_no_auth_without_login_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "@local"
auth = "api-key"

[model.aliases]
local = "ollama/local-model"

[internal_agents.compaction]
provider = "ollama"
model = "judge-model"

[internal_agents.session-title]
provider = "ollama"
model = "title-model"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.provider, "ollama");
    assert_eq!(config.auth, "none");
    assert_eq!(
        config
            .internal_agent_model("compaction")
            .unwrap()
            .expect_rho()
            .auth,
        "none"
    );
    assert_eq!(
        config
            .internal_agent_model("session-title")
            .unwrap()
            .expect_rho()
            .auth,
        "none"
    );
}

// Covers: a screen entry's `kind`, which picks the protocol a model on
// Ollama is asked over, and its allow threshold survive save and reload; an
// unset key stays unset so the entry keeps its default.
// Owner: config persistence
#[test]
fn screen_model_settings_round_trip_through_save() {
    use crate::config::ModelKind;
    use crate::permission_classifier::DECISION_SCREEN_ID;
    let cases = [
        (Some(ModelKind::Text), None),
        (Some(ModelKind::Decision), Some(90)),
        (None, None),
    ];
    for (kind, allow_threshold_percent) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        let mut screen = crate::config::InternalAgentModelConfig::new(
            "ollama".into(),
            "gemma4:31b".into(),
            "none".into(),
        );
        screen.expect_rho_mut().kind = kind;
        screen.expect_rho_mut().allow_threshold_percent = allow_threshold_percent;
        config.set_internal_agent_model_config(DECISION_SCREEN_ID, screen.clone());

        config.save(Some(path.clone())).unwrap();
        let reloaded = Config::load(Some(path)).unwrap();

        assert_eq!(
            reloaded.internal_agent_model(DECISION_SCREEN_ID).cloned(),
            Some(screen),
            "{kind:?} {allow_threshold_percent:?}"
        );
    }
}

// Covers: a delegating advisor must survive save and reload as a delegating
// advisor, and must not persist a Rho provider or auth it does not have.
// Owner: config persistence
#[test]
fn a_claude_cli_advisor_round_trips_through_save() {
    for model in [Some("opus".to_string()), None] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config {
            advisor_mode: true,
            ..Default::default()
        };
        let mut advisor = crate::config::InternalAgentModelConfig::claude_cli(model.clone());
        advisor.reasoning = Some(rho_providers::reasoning::ReasoningLevel::High);
        config.set_internal_agent_model_config(crate::agent::ADVISOR_AGENT_ID, advisor.clone());

        config.save(Some(path.clone())).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        let saved_toml: toml::Value = toml::from_str(&saved).unwrap();
        let advisor_table = saved_toml["internal_agents"]["advisor"]
            .as_table()
            .expect("serialized advisor table");
        assert_eq!(
            advisor_table.get("runtime").and_then(toml::Value::as_str),
            Some("claude-cli")
        );
        assert!(!advisor_table.contains_key("provider"), "{saved}");
        assert!(!advisor_table.contains_key("auth"), "{saved}");

        let reloaded = Config::load(Some(path)).unwrap();
        assert_eq!(
            reloaded
                .internal_agent_model(crate::agent::ADVISOR_AGENT_ID)
                .cloned(),
            Some(advisor),
            "model {model:?} did not round trip"
        );
    }
}

// Covers: `[internal_agents.compaction]` fails to route text-summary
// compaction to the configured model, loses an alias, ignores reasoning, or
// drops the config's providers, which a custom host needs to be found; an
// unset entry must keep the session model.
// Owner: config to compaction wiring
#[test]
fn compaction_internal_agent_selects_the_summarizer_model() {
    use crate::compaction::{CompactionConfig, SummarizerModel};
    use rho_sdk::ReasoningLevel;

    for (case, entry, expected) in [
        ("unset", "", None),
        (
            "alias with default reasoning",
            "[internal_agents.compaction]\nmodel = \"@cheap\"\n",
            Some(SummarizerModel {
                providers: Default::default(),
                provider: "anthropic".into(),
                model: "claude-haiku-4-5".into(),
                auth: "anthropic-api-key".into(),
                reasoning: ReasoningLevel::Low,
            }),
        ),
        (
            "explicit reasoning",
            "[internal_agents.compaction]\nprovider = \"openai\"\nmodel = \"gpt-5.5-mini\"\nauth = \"api-key\"\nreasoning = \"medium\"\n",
            Some(SummarizerModel {
                providers: Default::default(),
                provider: "openai".into(),
                model: "gpt-5.5-mini".into(),
                auth: "api-key".into(),
                reasoning: ReasoningLevel::Medium,
            }),
        ),
        (
            "custom host",
            "[providers.custom.summhost]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n[internal_agents.compaction]\nprovider = \"summhost\"\nmodel = \"local-model\"\nauth = \"none\"\n",
            Some(SummarizerModel {
                providers: Default::default(),
                provider: "summhost".into(),
                model: "local-model".into(),
                auth: "none".into(),
                reasoning: ReasoningLevel::Low,
            }),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!(
                "[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\n\n[model.aliases]\ncheap = \"anthropic/claude-haiku-4-5\"\n\n{entry}"
            ),
        )
        .unwrap();

        let config = Config::load(Some(path)).unwrap();
        // Every summarizer is built on the providers of its config.
        let expected = expected.map(|model| SummarizerModel {
            providers: config.providers.clone(),
            ..model
        });

        assert_eq!(CompactionConfig::from(&config).summarizer, expected, "{case}");
    }
}

// Covers: every setting the registry says applies at a loaded session's
// next turn is one `take_next_turn_settings` carries, so the schema never
// promises a change a running session does not take.
// Owner: config registry.
#[test]
fn next_turn_settings_carry_every_next_turn_setting() {
    use super::{registry::AGENT_SETTINGS, setting::Apply};
    let global = Config::parse_settings(
        r#"
[behavior]
advisor_mode = true

[codemode]
mode = "only"

[compaction]
auto_compact = false
compact_threshold_percent = 70
compact_target_percent = 30

[internal_agents.advisor]
provider = "anthropic"
model = "claude-opus-4-8"
reasoning = "high"
"#,
    )
    .unwrap();
    let next_turn = AGENT_SETTINGS
        .iter()
        .filter(|setting| setting.applies() == Apply::NextTurn)
        .collect::<Vec<_>>();
    let session = Config::default();
    // The fixture sets each one, so the check below is not vacuous.
    let unset = next_turn
        .iter()
        .filter(|setting| !setting.differs(&session, &global))
        .map(|setting| setting.key())
        .collect::<Vec<_>>();
    assert_eq!(
        unset,
        Vec::<&str>::new(),
        "the fixture leaves these at their defaults"
    );

    let mut session = session;
    let changes = session.take_next_turn_settings(&Config::default(), &global);
    let missed = next_turn
        .iter()
        .filter(|setting| setting.differs(&session, &global))
        .map(|setting| setting.key())
        .collect::<Vec<_>>();
    assert_eq!(missed, Vec::<&str>::new());
    assert_eq!(
        changes,
        super::NextTurnChanges {
            advisor: true,
            compaction: true,
            codemode: true,
        }
    );
}

// Covers: a loaded session takes only the next-turn settings the global
// config changed: a re-read that changed nothing, or changed only other
// settings, keeps the session's own values, such as its agent's; a group
// that changed globally replaces the session's (#1720).
// Owner: config registry.
#[test]
fn next_turn_settings_take_only_what_changed_globally() {
    let parse = |text: &str| Config::parse_settings(text).unwrap();
    let was = parse("[compaction]\ncompact_threshold_percent = 70\n");
    // The session's agent runs its own threshold and the advisor.
    let own =
        parse("[behavior]\nadvisor_mode = true\n[compaction]\ncompact_threshold_percent = 60\n");
    let threshold = |config: &Config| (config.advisor_mode, config.compact_threshold_percent);
    let take = |now: &Config| {
        let mut session = own.clone();
        let changes = session.take_next_turn_settings(&was, now);
        (threshold(&session), changes)
    };
    let changed = |advisor, compaction| super::NextTurnChanges {
        advisor,
        compaction,
        codemode: false,
    };

    assert_eq!(
        [
            take(&was.clone()),
            take(&parse(
                "[behavior]\nrtk = false\n[compaction]\ncompact_threshold_percent = 70\n"
            )),
            take(&parse("[compaction]\ncompact_threshold_percent = 80\n")),
        ],
        [
            ((true, 60), changed(false, false)),
            ((true, 60), changed(false, false)),
            ((true, 80), changed(false, true)),
        ]
    );
}
