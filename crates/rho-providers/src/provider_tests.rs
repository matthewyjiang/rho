#[test]
fn provider_ids_have_unique_descriptors_and_lookup_round_trips() {
    let providers = super::builtin_providers();

    for (index, descriptor) in providers.iter().enumerate() {
        assert_eq!(
            super::provider_descriptor(descriptor.name),
            Some(descriptor)
        );
        assert_eq!(super::provider_descriptor_by_id(descriptor.id), descriptor);
        assert!(providers[..index]
            .iter()
            .all(|other| { other.id != descriptor.id && other.name != descriptor.name }));
        assert!(
            !descriptor.auth_modes.is_empty(),
            "{} must declare at least one auth mode",
            descriptor.name
        );
    }
}

#[test]
fn poolside_model_id_codec_canonicalizes_and_expands_wire_ids() {
    use super::{ModelIdCodec, ProviderId};

    let poolside = super::provider_descriptor_by_id(ProviderId::Poolside);
    assert_eq!(poolside.model_id_codec, ModelIdCodec::ProviderPrefixed);
    assert_eq!(poolside.canonicalize_model_id("laguna-m.1"), "laguna-m.1");
    assert_eq!(
        poolside.canonicalize_model_id("poolside/laguna-m.1"),
        "laguna-m.1"
    );
    assert_eq!(
        poolside.canonicalize_model_id("poolside/poolside/laguna-m.1"),
        "laguna-m.1"
    );
    assert_eq!(poolside.wire_model_id("laguna-m.1"), "poolside/laguna-m.1");
    assert_eq!(
        poolside.wire_model_id("poolside/laguna-m.1"),
        "poolside/laguna-m.1"
    );
    assert_eq!(
        super::model_reference("poolside", "laguna-m.1"),
        "poolside/laguna-m.1"
    );
}

#[test]
fn openrouter_auth_modes_share_one_provider_and_legacy_aliases_normalize() {
    use super::ProviderId;

    let openrouter = super::provider_descriptor_by_id(ProviderId::OpenRouter);
    assert!(matches!(
        openrouter.runtime,
        super::ProviderRuntime::OpenAiCompatible { .. }
    ));
    assert!(openrouter.auth_mode("openrouter-api-key").is_some());
    assert!(openrouter.auth_mode("openrouter-oauth").is_some());

    assert!(super::provider_descriptor("openrouter-oauth").is_none());
    let resolved = super::resolve_provider_reference("openrouter-oauth").unwrap();
    assert_eq!(resolved.provider, openrouter);
    assert_eq!(resolved.auth_id(), "openrouter-oauth");

    let resolved = super::resolve_profile("openrouter", "openrouter-oauth").unwrap();
    assert_eq!(resolved.provider, openrouter);
    assert_eq!(resolved.auth_id(), "openrouter-oauth");

    let resolved = super::resolve_profile("openrouter-oauth", "openrouter-api-key").unwrap();
    assert_eq!(resolved.provider, openrouter);
    assert_eq!(resolved.auth_id(), "openrouter-oauth");
}

#[test]
fn xai_legacy_provider_alias_selects_oauth_mode() {
    let resolved = super::resolve_profile("xai-oauth", "xai-api-key").unwrap();

    assert_eq!(resolved.provider_name(), "xai");
    assert_eq!(resolved.auth_id(), "xai-oauth");
}

// Covers: exact profile resolution rejects cross-provider auth instead of soft fallback
// Owner: provider registry
#[test]
fn resolve_profile_exact_rejects_incompatible_auth() {
    let err = super::resolve_profile_exact("xai", "anthropic-api-key").unwrap_err();
    assert!(matches!(
        err,
        super::ProfileResolutionError::AuthNotValidForProvider { .. }
    ));
    assert!(!super::provider_accepts_auth("xai", "anthropic-api-key"));
    assert!(super::provider_accepts_auth("xai", "xai-oauth"));

    // Soft resolve still falls back to the provider default.
    let soft = super::resolve_profile("xai", "anthropic-api-key").unwrap();
    assert_eq!(soft.provider_name(), "xai");
    assert_eq!(soft.auth_id(), "xai-api-key");
}

// Covers: qwen-token-plan must resolve as OpenAI-compatible with api-key auth
// Owner: provider registry
#[test]
fn qwen_token_plan_is_openai_compatible_with_api_key_auth() {
    use super::{
        CatalogConstruction, CatalogReasoningPolicy, ProviderId, ProviderRuntime,
        QWEN_TOKEN_PLAN_API_BASE,
    };
    use crate::model::registry::provider_runtime;
    use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

    let descriptor = super::provider_descriptor_by_id(ProviderId::QwenTokenPlan);
    assert_eq!(descriptor.name, "qwen-token-plan");
    assert_eq!(descriptor.metadata_upstream, "alibaba-token-plan");
    assert_eq!(
        descriptor.catalog_reasoning,
        CatalogReasoningPolicy::ExactAdvertised
    );
    assert!(descriptor.auth_mode("qwen-token-plan-api-key").is_some());
    assert_eq!(
        provider_runtime("qwen-token-plan"),
        Some(ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::QwenTokenPlan,
            default_api_base: QWEN_TOKEN_PLAN_API_BASE,
            catalog_construction: CatalogConstruction::Runtime,
        })
    );
}

// Covers: meta must resolve as OpenAI-compatible with api-key auth and default model
// Owner: provider registry
#[test]
fn meta_is_openai_compatible_with_api_key_auth() {
    use super::{
        CatalogConstruction, CatalogReasoningPolicy, ProviderId, ProviderRuntime, META_API_BASE,
    };
    use crate::model::registry::provider_runtime;
    use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

    let descriptor = super::provider_descriptor_by_id(ProviderId::Meta);
    assert_eq!(descriptor.name, "meta");
    assert_eq!(descriptor.display_name, "Meta Model API");
    assert_eq!(descriptor.metadata_upstream, "meta");
    assert_eq!(descriptor.default_model, Some("muse-spark-1.2"));
    assert_eq!(
        descriptor.catalog_reasoning,
        CatalogReasoningPolicy::ExactAdvertised
    );
    assert!(descriptor.auth_mode("meta-api-key").is_some());
    assert_eq!(
        provider_runtime("meta"),
        Some(ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: META_API_BASE,
            catalog_construction: CatalogConstruction::Runtime,
        })
    );
}

// Covers: opencode-go must resolve as OpenAI-compatible with catalog npm construction
// Owner: provider registry
#[test]
fn opencode_go_is_openai_compatible_with_catalog_npm_construction() {
    use super::{
        CatalogConstruction, CatalogReasoningPolicy, ProviderId, ProviderRuntime,
        OPENCODE_GO_API_BASE,
    };
    use crate::model::registry::provider_runtime;
    use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

    let descriptor = super::provider_descriptor_by_id(ProviderId::OpenCodeGo);
    assert_eq!(descriptor.name, "opencode-go");
    assert_eq!(descriptor.display_name, "OpenCode Go");
    assert_eq!(descriptor.metadata_upstream, "opencode-go");
    assert_eq!(descriptor.default_model, None);
    assert_eq!(
        descriptor.catalog_reasoning,
        CatalogReasoningPolicy::ExactAdvertised
    );
    assert!(descriptor.auth_mode("opencode-go-api-key").is_some());
    assert_eq!(
        provider_runtime("opencode-go"),
        Some(ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OPENCODE_GO_API_BASE,
            catalog_construction: CatalogConstruction::PreferModelsDevNpm,
        })
    );
}

// Covers: custom-host persistence omits Chat Completions and Anthropic Messages
// Owner: provider registry
#[test]
fn persisted_custom_host_value_omits_non_custom_apis() {
    use super::OpenAiCompatibleApi;

    assert_eq!(
        OpenAiCompatibleApi::ChatCompletions.persisted_custom_host_value(),
        None
    );
    assert_eq!(
        OpenAiCompatibleApi::Responses.persisted_custom_host_value(),
        Some("responses")
    );
    assert_eq!(
        OpenAiCompatibleApi::AnthropicMessages.persisted_custom_host_value(),
        None
    );
}

// Covers: MiniMax declares Anthropic Messages on its compatible base
// Owner: provider registry
#[test]
fn minimax_declares_anthropic_messages_on_its_compatible_base() {
    use super::{
        CatalogConstruction, CatalogReasoningPolicy, OpenAiCompatibleApi, ProviderId,
        ProviderModelRefreshKind, ProviderRuntime, MINIMAX_API_BASE,
    };
    use crate::model::registry::provider_runtime;
    use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

    let descriptor = super::provider_descriptor_by_id(ProviderId::MiniMax);
    assert_eq!(descriptor.name, "minimax");
    assert_eq!(descriptor.display_name, "MiniMax");
    assert_eq!(descriptor.metadata_upstream, "minimax");
    assert_eq!(descriptor.default_model, Some("MiniMax-M3"));
    assert_eq!(
        descriptor.catalog_reasoning,
        CatalogReasoningPolicy::ExactAdvertised
    );
    assert!(descriptor.auth_mode("minimax-api-key").is_some());
    assert_eq!(
        descriptor.openai_compatible_api(),
        OpenAiCompatibleApi::AnthropicMessages
    );
    assert_eq!(
        descriptor.model_refresh,
        Some(ProviderModelRefreshKind::AnthropicCompatible)
    );
    assert_eq!(
        provider_runtime("minimax"),
        Some(ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: MINIMAX_API_BASE,
            catalog_construction: CatalogConstruction::Runtime,
        })
    );
}

// Covers: openai api-key and codex share a runtime family for auth resolution
// Owner: provider registry
#[test]
fn openai_and_codex_share_runtime_family() {
    use super::{OpenAiRuntimeAuth, ProviderId, ProviderRuntime};

    let openai = super::provider_descriptor_by_id(ProviderId::OpenAi);
    let codex = super::provider_descriptor_by_id(ProviderId::OpenAiCodex);
    assert!(openai.shares_auth_family(*codex));
    assert_eq!(
        openai.runtime,
        ProviderRuntime::OpenAi {
            auth_mode: OpenAiRuntimeAuth::ApiKey,
        }
    );
    assert_eq!(
        codex.runtime,
        ProviderRuntime::OpenAi {
            auth_mode: OpenAiRuntimeAuth::Codex,
        }
    );
}
