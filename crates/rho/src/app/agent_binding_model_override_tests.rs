use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_providers::credentials::{
    save_codex_tokens, save_provider_api_key, save_xai_tokens, CodexTokens, MemoryCredentialStore,
    XaiTokens,
};

use super::super::{AgentBinder, AgentInvocation, AgentRole, ModelOverride};
use crate::{
    agent::{
        AgentCapabilities, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, ModelSelection,
        PromptPolicy, ToolCapability, ToolPolicy,
    },
    config::Config,
    permission::PermissionMode,
};

fn definition(model: ModelPolicy) -> Arc<AgentDefinition> {
    Arc::new(AgentDefinition {
        id: AgentId::new("review-fixture").unwrap(),
        description: "review".into(),
        prompt: PromptPolicy::Replace("Only review, never edit.".into()),
        runtime: AgentRuntimeSpec::Rho {
            model,
            tools: ToolPolicy::Allow([ToolCapability::ReadFile].into()),
            reasoning: Some(rho_sdk::ReasoningLevel::Low),
        },
    })
}

fn invocation() -> AgentInvocation {
    AgentInvocation {
        role: AgentRole::Delegated,
        available_tools: AgentCapabilities::all_host_tools(),
    }
}

// Covers: a call override replaces every definition selection without altering
// identity, prompt, tools, permission ceiling, reasoning, or the parent's config.
// Owner: agent binding; existing tests cover ordinary model-policy binding.
#[test]
fn model_override_replaces_pins_without_widening_authority() {
    let host = Config {
        provider: "openai-codex".into(),
        model: "gpt-6.1-sol".into(),
        auth: "codex".into(),
        permission_mode: PermissionMode::Plan,
        fast_mode: true,
        ..Config::default()
    };
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "xai-api-key", "fixture-key").unwrap();
    let pin = ModelSelection {
        provider: Some("invalid-provider".into()),
        model: "invalid-pinned-model".into(),
        auth: Some("invalid-auth".into()),
        fast: false,
    };
    for policy in [
        ModelPolicy::Inherit,
        ModelPolicy::Prefer(pin.clone()),
        ModelPolicy::Require(pin.clone()),
        ModelPolicy::Select(pin),
    ] {
        let definition = definition(policy);
        let bound = AgentBinder::bind_with_credentials(
            definition.clone(),
            invocation(),
            &host,
            Some(ModelOverride::Reference("xai/grok-4.7")),
            &store,
        )
        .unwrap();
        let config = bound.rho_config().unwrap();
        assert_eq!(
            (
                config.provider.as_str(),
                config.model.as_str(),
                config.auth.as_str(),
                config.permission_mode,
                config.reasoning,
                config.fast_mode
            ),
            (
                "xai",
                "grok-4.7",
                "xai-api-key",
                PermissionMode::Plan,
                rho_sdk::ReasoningLevel::Low,
                false
            ),
        );
        assert_eq!(bound.definition(), definition.as_ref());
        assert_eq!(bound.fingerprint(), definition.fingerprint());
        assert_eq!(
            bound.rho_capabilities(),
            Some(&AgentCapabilities::new([ToolCapability::ReadFile].into()))
        );
        assert_eq!(
            (
                bound.artifact_identity().provider,
                bound.artifact_identity().model
            ),
            ("xai".into(), Some("grok-4.7".into())),
        );
    }
    assert_eq!(
        (
            host.provider.as_str(),
            host.model.as_str(),
            host.auth.as_str(),
            host.fast_mode
        ),
        ("openai-codex", "gpt-6.1-sol", "codex", true)
    );
}

// Covers: aliases and concrete references share catalog/auth resolution;
// compatible parent OAuth survives, foreign auth does not leak across families.
// Owner: agent model selection.
#[test]
fn model_override_resolves_aliases_and_provider_auth() {
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "xai-api-key", "fixture-key").unwrap();
    save_xai_tokens(
        &store,
        &XaiTokens {
            access_token: "fixture".into(),
            refresh_token: None,
            expires_at_unix: None,
            id_token: None,
        },
    )
    .unwrap();
    save_codex_tokens(
        &store,
        &CodexTokens {
            access_token: "fixture".into(),
            refresh_token: None,
            id_token: None,
            account_id: None,
        },
    )
    .unwrap();
    let mut host = Config {
        provider: "xai".into(),
        model: "grok-4.5".into(),
        auth: "xai-oauth".into(),
        ..Config::default()
    };
    host.model_aliases = crate::model_aliases::ModelAliases::from_entries(
        [
            ("review".into(), "openai-codex/gpt-6.1-sol".into()),
            ("local".into(), "grok-4.7".into()),
        ]
        .into(),
    )
    .unwrap();
    for (reference, expected) in [
        ("xai/grok-4.7", ("xai", "grok-4.7", "xai-oauth", None)),
        ("grok-4.7", ("xai", "grok-4.7", "xai-oauth", None)),
        ("@local", ("xai", "grok-4.7", "xai-oauth", Some("local"))),
        (
            "@review",
            ("openai-codex", "gpt-6.1-sol", "codex", Some("review")),
        ),
    ] {
        let bound = AgentBinder::bind_with_credentials(
            definition(ModelPolicy::Inherit),
            invocation(),
            &host,
            Some(ModelOverride::Reference(reference)),
            &store,
        )
        .unwrap();
        let config = bound.rho_config().unwrap();
        assert_eq!(
            (
                config.provider.as_str(),
                config.model.as_str(),
                config.auth.as_str(),
                config.model_alias.as_deref()
            ),
            expected,
            "{reference}"
        );
    }
}

// Covers: invalid catalog selections and external-runtime
// selections fail instead of silently running the parent or a definition pin.
// Owner: agent binding.
#[test]
fn model_override_rejects_unavailable_selections_and_external_runtimes() {
    let host = Config {
        permission_mode: PermissionMode::Plan,
        ..Config::default()
    };
    let store = MemoryCredentialStore::default();
    for reference in ["", "unknown/model", "xai/not-a-model", "@undefined"] {
        assert!(
            AgentBinder::bind_with_credentials(
                definition(ModelPolicy::Inherit),
                invocation(),
                &host,
                Some(ModelOverride::Reference(reference)),
                &store
            )
            .is_err(),
            "{reference}"
        );
    }
    for runtime in [
        AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            tools: crate::agent::ClaudeToolPolicy::Allow(vec!["Read".into()]),
            inherit_claude_config: false,
            model: None,
            reasoning: None,
        }),
        AgentRuntimeSpec::Cursor(crate::agent::CursorAgentConfig {
            tools: vec![crate::agent::CursorTool::Read],
            model: None,
        }),
        AgentRuntimeSpec::Antigravity(crate::agent::AntigravityAgentConfig {
            tools: vec![crate::agent::AntigravityTool::ViewFile],
            model: None,
        }),
    ] {
        let mut definition = definition(ModelPolicy::Inherit).as_ref().clone();
        definition.runtime = runtime;
        definition.prompt = PromptPolicy::Extend("review".into());
        assert!(AgentBinder::bind_with_credentials(
            Arc::new(definition.clone()),
            invocation(),
            &host,
            /*model_override*/ None,
            &store,
        )
        .is_ok());
        assert!(AgentBinder::bind_with_credentials(
            Arc::new(definition),
            invocation(),
            &host,
            Some(ModelOverride::Reference("xai/grok-4.7")),
            &store
        )
        .is_err());
    }
}
