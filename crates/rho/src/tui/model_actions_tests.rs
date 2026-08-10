use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_providers::model::catalog::ModelSelection;
use rho_providers::reasoning::ReasoningLevel;

use super::super::InteractiveModelSelection;
use crate::{
    agent::ADVISOR_AGENT_ID, config::InternalAgentModelConfig, model_aliases::ModelAliases,
    tui::tests::test_app,
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

    let mut agent =
        test_edit_tool_runtime(EditTool::Pinned(rho_tools::EditFormat::Hashline)).await;
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
