use std::collections::BTreeMap;
use std::time::Instant;

use pretty_assertions::assert_eq;
use rho_providers::model::catalog::ModelSelection;
use rho_providers::model::provider_models::with_provider_models_cache_dir_for_tests;
use rho_providers::reasoning::ReasoningLevel;

use super::super::InteractiveModelSelection;
use crate::{
    agent::ADVISOR_AGENT_ID,
    commands::parse_command,
    config::InternalAgentModelConfig,
    model_aliases::ModelAliases,
    tui::{tests::test_app, ComposerMode, Entry, StreamKind},
};

#[test]
fn model_refresh_prefers_the_active_available_auth_mode() {
    let descriptor = rho_providers::provider::provider_descriptor("openrouter").unwrap();
    let available = vec!["openrouter-api-key".into(), "openrouter-oauth".into()];

    assert_eq!(
        super::refresh_auth_for_provider(descriptor, "openrouter-oauth", &available),
        "openrouter-oauth"
    );
}

// Covers: /config refresh must send a stored key instead of probing anonymously
// Owner: model list refresh
#[test]
fn model_refresh_prefers_a_stored_key_over_keyless() {
    let descriptor = rho_providers::provider::provider_descriptor("ollama").unwrap();
    let available = vec!["none".into(), "ollama-api-key".into()];

    assert_eq!(
        super::refresh_auth_for_provider(descriptor, "none", &available),
        "ollama-api-key"
    );
}

#[test]
fn model_refresh_falls_back_to_an_available_auth_mode() {
    let descriptor = rho_providers::provider::provider_descriptor("openrouter").unwrap();
    let available = vec!["openrouter-oauth".into()];

    assert_eq!(
        super::refresh_auth_for_provider(descriptor, "openrouter-api-key", &available),
        "openrouter-oauth"
    );
}

fn aliases(entries: &[(&str, &str)]) -> ModelAliases {
    ModelAliases::from_entries(
        entries
            .iter()
            .map(|(name, target)| (name.to_string(), target.to_string()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap()
}

#[test]
fn resolves_alias_before_interactive_model_lookup() {
    let mut app = test_app();
    app.info.runtime.model_aliases = aliases(&[("deep", "openai-codex/gpt-5.5")]);

    let resolved = app
        .resolve_model_selection("@deep", &app.info.runtime.provider, &app.info.runtime.auth)
        .unwrap();

    assert_eq!(
        resolved,
        InteractiveModelSelection {
            selection: ModelSelection {
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                auth: "codex".into(),
                from_catalog: true,
            },
            alias: Some("deep".into()),
        }
    );
}

#[test]
fn bare_alias_keeps_current_provider() {
    let mut app = test_app();
    app.info.runtime.model_aliases = aliases(&[("fast", "gpt-5.5")]);

    let resolved = app
        .resolve_model_selection("@fast", "openai-codex", "codex")
        .unwrap();

    assert_eq!(
        resolved,
        InteractiveModelSelection {
            selection: ModelSelection {
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                auth: "codex".into(),
                from_catalog: true,
            },
            alias: Some("fast".into()),
        }
    );
}

#[test]
fn reports_undefined_alias_in_interactive_model_lookup() {
    let app = test_app();

    let error = app
        .resolve_model_selection(
            "@missing",
            &app.info.runtime.provider,
            &app.info.runtime.auth,
        )
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
    );
}

// Covers: first internal-agent model pick does not materialize definition defaults.
// Owner: internal agent model selection
#[test]
fn selecting_an_internal_agent_model_keeps_reasoning_unset_by_default() {
    let mut app = test_app();

    app.select_internal_agent_model(
        ADVISOR_AGENT_ID,
        Some(ModelSelection {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            auth: "api-key".into(),
            from_catalog: true,
        }),
    )
    .unwrap();

    let stored = app
        .info
        .runtime
        .internal_agents
        .get(ADVISOR_AGENT_ID)
        .expect("advisor model stored");
    assert_eq!(stored.reasoning, None);
    assert_eq!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .internal_agent_model(ADVISOR_AGENT_ID)
            .and_then(|model| model.reasoning),
        None
    );
}

// Covers: an explicit reasoning override is carried across model switches.
// Owner: internal agent model selection
#[test]
fn selecting_an_internal_agent_model_carries_explicit_reasoning() {
    let mut app = test_app();
    let mut previous =
        InternalAgentModelConfig::new("anthropic".into(), "claude-old".into(), "api-key".into());
    previous.reasoning = Some(ReasoningLevel::High);
    app.info
        .runtime
        .internal_agents
        .insert(ADVISOR_AGENT_ID.into(), previous);

    app.select_internal_agent_model(
        ADVISOR_AGENT_ID,
        Some(ModelSelection {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            auth: "api-key".into(),
            from_catalog: true,
        }),
    )
    .unwrap();

    assert_eq!(
        app.info
            .runtime
            .internal_agents
            .get(ADVISOR_AGENT_ID)
            .and_then(|model| model.reasoning),
        Some(ReasoningLevel::High)
    );
}

// Covers: select_model_report must apply Auto's provider-preferred edit format
// after a provider change, while a pinned preference stays put.
// Owner: model switch edit-tool handoff
#[tokio::test]
async fn select_model_report_auto_edit_tool_follows_provider_change() {
    use std::sync::Arc;

    use rho_providers::credentials::{save_provider_api_key, MemoryCredentialStore};

    use crate::{
        app::interactive_runtime::test_edit_tool_runtime,
        config::EditTool,
        tui::{tests::test_bootstrap, App, InteractiveRuntime},
    };

    async fn switch_to_anthropic(app: &mut App, agent: &mut InteractiveRuntime) {
        app.select_model_report(
            InteractiveModelSelection {
                selection: ModelSelection {
                    provider: "anthropic".into(),
                    model: "claude-fable-5".into(),
                    auth: "api-key".into(),
                    from_catalog: true,
                },
                alias: None,
            },
            agent,
        )
        .await
        .expect("model switch should succeed")
        .expect("handoff report");
    }

    fn advertised_edit_name(agent: &InteractiveRuntime) -> Option<&'static str> {
        ["edit", "apply_patch", "str_replace"]
            .into_iter()
            .find(|name| agent.has_tool(name))
    }

    // --- Auto follows the provider ---
    let store = Arc::new(MemoryCredentialStore::default());
    save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
    save_provider_api_key(store.as_ref(), "anthropic", "sk-ant-test").unwrap();
    let mut app = App::new_with_credentials(
        test_bootstrap(),
        store,
        crate::herdr::HerdrGraphicsCapability::NotHerdr,
        crate::tools::mcp::McpSessionReport::default(),
        crate::tools::mcp::McpCatalog::default(),
        crate::plugins::PluginLoadReport::default(),
    );
    app.info
        .services
        .config_repository
        .update(|config| {
            config.edit_tool = EditTool::Auto;
            config.provider = "openai".into();
            config.model = "gpt-5.5".into();
            config.auth = "api-key".into();
        })
        .unwrap();

    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    assert_eq!(
        advertised_edit_name(&agent),
        Some("edit"),
        "Auto + openai should start on hashline (`edit`)"
    );

    switch_to_anthropic(&mut app, &mut agent).await;
    assert_eq!(app.info.runtime.provider, "anthropic");
    assert_eq!(
        advertised_edit_name(&agent),
        Some("str_replace"),
        "Auto must follow anthropic to str_replace after select_model_report"
    );

    // --- Pinned does not follow ---
    let store = Arc::new(MemoryCredentialStore::default());
    save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
    save_provider_api_key(store.as_ref(), "anthropic", "sk-ant-test").unwrap();
    let mut app = App::new_with_credentials(
        test_bootstrap(),
        store,
        crate::herdr::HerdrGraphicsCapability::NotHerdr,
        crate::tools::mcp::McpSessionReport::default(),
        crate::tools::mcp::McpCatalog::default(),
        crate::plugins::PluginLoadReport::default(),
    );
    app.info
        .services
        .config_repository
        .update(|config| {
            config.edit_tool = EditTool::Pinned(rho_tools::EditFormat::Hashline);
            config.provider = "openai".into();
            config.model = "gpt-5.5".into();
            config.auth = "api-key".into();
        })
        .unwrap();

    let mut agent = test_edit_tool_runtime(EditTool::Pinned(rho_tools::EditFormat::Hashline)).await;
    assert_eq!(advertised_edit_name(&agent), Some("edit"));

    switch_to_anthropic(&mut app, &mut agent).await;
    assert_eq!(app.info.runtime.provider, "anthropic");
    assert_eq!(
        advertised_edit_name(&agent),
        Some("edit"),
        "pinned hashline must not follow the anthropic provider change"
    );
    assert!(!agent.has_tool("str_replace"));
}

// Covers: a mid-session model switch must reach the model as an appended line,
// because the system prompt names the starting model and then stays fixed. The
// line names only the new model, and includes the catalog name when the snapshot
// already has one. A first selection on an empty session is not a switch and
// must stay silent.
// Owner: model switch context notice
#[tokio::test]
async fn select_model_report_tells_the_model_about_a_mid_session_switch() {
    use std::sync::Arc;

    use rho_providers::{
        credentials::{save_provider_api_key, MemoryCredentialStore},
        model::{
            display_name::ModelDisplayNameCacheGuard,
            models_dev::{
                with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests,
                ModelMetadata,
            },
        },
    };

    use crate::{
        app::interactive_runtime::test_edit_tool_runtime,
        config::EditTool,
        model_identity::PromptModel,
        prompt::{model_switch_context, ModelSwitchKind},
        tui::{tests::test_bootstrap, App, InteractiveRuntime},
    };

    async fn switch_to_anthropic(app: &mut App, agent: &mut InteractiveRuntime) {
        app.select_model_report(
            InteractiveModelSelection {
                selection: ModelSelection {
                    provider: "anthropic".into(),
                    model: "claude-fable-5".into(),
                    auth: "api-key".into(),
                    from_catalog: true,
                },
                alias: None,
            },
            agent,
        )
        .await
        .expect("model switch should succeed");
    }

    /// Model-visible history as one string. A provider change also swaps the
    /// Auto edit tool, so the switch notice is not reliably the last message.
    fn history_text(agent: &InteractiveRuntime) -> String {
        agent
            .history()
            .iter()
            .filter_map(|message| match message {
                rho_sdk::model::Message::User(blocks) => Some(
                    blocks
                        .iter()
                        .filter_map(|block| match block {
                            rho_sdk::model::ContentBlock::Text(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app_on_openai() -> App {
        let store = Arc::new(MemoryCredentialStore::default());
        save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
        save_provider_api_key(store.as_ref(), "anthropic", "sk-ant-test").unwrap();
        let app = App::new_with_credentials(
            test_bootstrap(),
            store,
            crate::herdr::HerdrGraphicsCapability::NotHerdr,
            crate::tools::mcp::McpSessionReport::default(),
            crate::tools::mcp::McpCatalog::default(),
            crate::plugins::PluginLoadReport::default(),
        );
        app.info
            .services
            .config_repository
            .update(|config| {
                config.provider = "openai".into();
                config.model = "gpt-5.5".into();
                config.auth = "api-key".into();
            })
            .unwrap();
        app
    }

    let catalog = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
        let _names = ModelDisplayNameCacheGuard::new();
        write_cached_model_metadata_for_tests(
            "anthropic",
            "claude-fable-5",
            &ModelMetadata {
                display_name: Some("Claude Fable 5".into()),
                reasoning_metadata_complete: true,
                ..ModelMetadata::default()
            },
        );
    });

    // --- A started session is told, naming the new model with its catalog name ---
    let mut app = app_on_openai();
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    agent
        .append_user_context_with_display("first turn".into(), "first turn".into())
        .unwrap();

    // Keep the test cache dir for the switch so describe() reads the seeded name.
    let _cache =
        rho_providers::model::models_dev::ModelsDevCacheDirGuard::new(catalog.path().to_path_buf());
    let _names = ModelDisplayNameCacheGuard::new();
    switch_to_anthropic(&mut app, &mut agent).await;

    let text = history_text(&agent);
    let (stored, _) = model_switch_context(
        ModelSwitchKind::Conversation,
        &PromptModel::Rho {
            provider: "anthropic".into(),
            model: "claude-fable-5".into(),
        },
    );
    assert!(
        text.contains(stored.trim()),
        "expected switch notice {stored:?} in history:\n{text}"
    );
    // The model the session started on stays readable in the system prompt, so
    // the notice does not restate it.
    assert!(!text.contains("openai/gpt-5.5"), "{text}");

    // --- A first selection on an empty session stays silent ---
    let mut app = app_on_openai();
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    assert!(agent.history().is_empty());

    switch_to_anthropic(&mut app, &mut agent).await;

    let text = history_text(&agent);
    assert!(
        !text.contains("conversation model switched"),
        "a first model choice is not a switch: {text}"
    );
}

fn with_empty_provider_models_cache<T>(f: impl FnOnce() -> T) -> T {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), f)
}

fn last_notice(app: &crate::tui::App) -> &str {
    match app.history.last() {
        Some(Entry::Notice(text)) => text,
        other => panic!("expected a transcript notice, got {other:?}"),
    }
}

// Covers: empty /config model picker must persist a transcript notice, not toast-only
// Owner: tui model picker
#[test]
fn empty_model_picker_writes_a_transcript_notice() {
    with_empty_provider_models_cache(|| {
        let mut app = test_app();
        app.open_config_conversation_model_picker();

        assert!(
            matches!(
                app.history.entries(),
                [Entry::Notice(text)] if !text.is_empty()
            ),
            "empty model cache must leave a transcript notice: {:?}",
            app.history.entries()
        );
        assert!(!app.status().is_empty());
        assert!(app.status_overlay.is_some());
        assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    });
}

// Covers: repeating an empty /config model row must not stack identical notices
// Owner: tui model picker
#[test]
fn empty_model_picker_does_not_stack_duplicate_notices() {
    with_empty_provider_models_cache(|| {
        let mut app = test_app();
        app.open_config_conversation_model_picker();
        app.open_config_conversation_model_picker();

        assert!(
            matches!(app.history.entries(), [Entry::Notice(_)]),
            "a second empty picker press must re-toast, not append: {:?}",
            app.history.entries()
        );
        assert!(app.status_overlay.is_some());
    });
}

// Covers: wait-until-refresh copy must follow session busy, not only ProviderTurn
// Owner: tui model picker
#[test]
fn empty_model_picker_wait_clause_follows_busy_session() {
    with_empty_provider_models_cache(|| {
        let idle = {
            let mut app = test_app();
            app.open_config_conversation_model_picker();
            last_notice(&app).to_string()
        };
        let notices = [
            (
                "provider turn",
                (|app| app.begin_provider_turn_ui()) as fn(&mut crate::tui::App),
            ),
            ("compacting", |app| app.begin_compact_ui()),
            ("cancellable wait", |app| app.begin_cancellable_wait_ui()),
        ]
        .map(|(name, setup)| {
            let mut app = test_app();
            setup(&mut app);
            app.open_config_conversation_model_picker_during_turn();
            (name, last_notice(&app).to_string())
        });

        for (name, notice) in &notices {
            assert_ne!(
                notice, &idle,
                "{name} must keep the wait clause while refresh is blocked"
            );
        }
        assert_eq!(notices[0].1, notices[1].1);
        assert_eq!(notices[0].1, notices[2].1);
    });
}

// Covers: empty /model during a turn must flush the live assistant stream before the notice
// Owner: tui model picker
#[test]
fn empty_model_picker_during_turn_does_not_split_a_live_stream() {
    with_empty_provider_models_cache(|| {
        let mut app = test_app();
        app.begin_provider_turn_ui();
        app.streams.current_stream_kind = Some(StreamKind::Assistant);
        app.streams
            .push_delta(StreamKind::Assistant, "held assistant tail", Instant::now());

        let invocation = parse_command("/model").unwrap().unwrap();
        app.execute_model_command_during_turn(invocation).unwrap();

        assert!(
            matches!(
                app.history.entries(),
                [Entry::Assistant(assistant), Entry::Notice(notice)]
                    if assistant == "held assistant tail" && !notice.is_empty()
            ),
            "notice must follow the flushed assistant row, not split it: {:?}",
            app.history.entries()
        );
        assert!(app.streams.assistant_stream.pending_text().is_empty());
        assert_eq!(app.streams.current_stream_kind, None);
        assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
        assert!(app.status_overlay.is_some());
    });
}
