use std::collections::BTreeMap;
use std::time::Instant;

use pretty_assertions::assert_eq;

use rho_providers::model::catalog::ModelSelection;
use rho_providers::model::provider_models::with_provider_models_cache_dir_for_tests;
use rho_providers::reasoning::ReasoningLevel;

use super::super::InteractiveModelSelection;
use crate::{
    agent::ADVISOR_AGENT_ID,
    config::InternalAgentModelConfig,
    model_aliases::ModelAliases,
    tui::{tests::test_app, ComposerMode, Entry, StreamKind},
};

// Covers: /config refresh keeps the active auth when available, sends a stored
// key instead of probing anonymously, and otherwise falls back to an available mode
// Owner: model list refresh
#[test]
fn model_refresh_selects_an_available_auth_mode() {
    for (case, provider, preferred, available, expected) in [
        (
            "active available auth wins",
            "openrouter",
            "openrouter-oauth",
            &["openrouter-api-key", "openrouter-oauth"][..],
            "openrouter-oauth",
        ),
        (
            "stored key beats keyless",
            "ollama",
            "none",
            &["none", "ollama-api-key"][..],
            "ollama-api-key",
        ),
        (
            "falls back to an available mode",
            "openrouter",
            "openrouter-api-key",
            &["openrouter-oauth"][..],
            "openrouter-oauth",
        ),
    ] {
        let descriptor = rho_providers::provider::provider_descriptor(provider).unwrap();
        let available = available
            .iter()
            .map(|auth| auth.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            super::refresh_auth_for_provider(descriptor, preferred, &available),
            expected,
            "{case}"
        );
    }
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

// Covers: an alias resolves before catalog lookup; a bare alias target keeps
// the current provider and auth.
// Owner: interactive model selection
#[test]
fn resolves_aliases_before_interactive_model_lookup() {
    for (alias, target, provider, auth) in [
        ("deep", "openai-codex/gpt-5.5", "openai", "api-key"),
        ("fast", "gpt-5.5", "openai-codex", "codex"),
    ] {
        let mut app = test_app();
        app.info.runtime.model_aliases = aliases(&[(alias, target)]);

        let resolved = app
            .resolve_model_selection(&format!("@{alias}"), provider, auth)
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
                alias: Some(alias.into()),
            },
            "@{alias}"
        );
    }
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

fn with_empty_provider_models_cache<T>(f: impl FnOnce() -> T) -> T {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), f)
}

/// Opens the conversation model picker while idle: with no cached models,
/// it reports them missing instead.
fn open_idle_model_picker(app: &mut crate::tui::App) {
    app.refresh_available_auths();
    if app.conversation_model_picker().items.is_empty() {
        app.report_missing_cached_provider_models();
    }
}

/// [`open_idle_model_picker`] while a turn runs.
fn open_model_picker_during_turn(app: &mut crate::tui::App) {
    app.refresh_available_auths();
    if app.conversation_model_picker_during_run().items.is_empty() {
        app.report_missing_cached_provider_models();
    }
}

fn last_notice(app: &crate::tui::App) -> &str {
    if let Some(notice) = app.streams.pending_notices.last() {
        return notice;
    }
    match app.history.last() {
        Some(Entry::Notice(text)) => text,
        other => panic!("expected a transcript notice, got {other:?}"),
    }
}

// Covers: an empty /model picker persists one transcript notice (not a
// toast only), and repeating the row re-toasts instead of stacking notices.
// Owner: tui model picker
#[test]
fn empty_model_picker_leaves_one_transcript_notice() {
    with_empty_provider_models_cache(|| {
        let mut app = test_app();
        open_idle_model_picker(&mut app);

        assert!(
            matches!(
                app.history.entries(),
                [Entry::Notice(text)] if !text.is_empty()
            ),
            "empty model cache must leave a transcript notice: {:?}",
            app.history.entries()
        );
        assert!(!app.status().is_empty());
        assert!(matches!(app.input_ui.composer(), ComposerMode::Input));

        open_idle_model_picker(&mut app);

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
            open_idle_model_picker(&mut app);
            last_notice(&app).to_string()
        };
        let mut app = test_app();
        app.begin_provider_turn_ui();
        open_model_picker_during_turn(&mut app);
        assert!(app.streams.pending_notices.is_empty());
        assert_ne!(
            last_notice(&app),
            idle.as_str(),
            "a provider turn must keep the wait clause while refresh is blocked"
        );
    });
}

// Covers: repeated empty /model during a turn queues one notice after the complete stream
// Owner: tui model picker
#[test]
fn empty_model_picker_during_turn_does_not_split_a_live_stream() {
    with_empty_provider_models_cache(|| {
        let mut app = test_app();
        app.begin_provider_turn_ui();
        app.streams.current_stream_kind = Some(StreamKind::Assistant);
        app.streams
            .push_delta(StreamKind::Assistant, "held assistant tail", Instant::now());

        open_model_picker_during_turn(&mut app);
        open_model_picker_during_turn(&mut app);

        assert!(app.history.entries().is_empty());
        assert_eq!(app.streams.pending_notices.len(), 1);
        app.streams
            .push_delta(StreamKind::Assistant, " continues", Instant::now());
        app.finish_streams();
        assert!(app.streams.pending_notices.is_empty());

        assert!(
            matches!(
                app.history.entries(),
                [Entry::Assistant(assistant), Entry::Notice(notice)]
                    if assistant.text == "held assistant tail continues" && !notice.is_empty()
            ),
            "notice must follow the flushed assistant row, not split it: {:?}",
            app.history.entries()
        );
        assert!(app.streams.assistant_stream.pending_text().is_empty());
        assert_eq!(app.streams.current_stream_kind, None);
        assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
        assert!(app.status_overlay.is_some());

        // Already-emitted text is retained for final-answer reconciliation,
        // but must not make a later notice wait for another stream boundary.
        app.insert_entry(&Entry::Notice("after stream".into()));
        assert!(app.streams.pending_notices.is_empty());
        assert!(matches!(app.history.last(), Some(Entry::Notice(text)) if text == "after stream"));
    });
}
