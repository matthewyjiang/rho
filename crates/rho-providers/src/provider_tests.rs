#[test]
fn provider_ids_have_unique_descriptors_and_lookup_round_trips() {
    let providers = super::providers();

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
    use super::{ProviderId, RuntimeProviderId};

    let openrouter = super::provider_descriptor_by_id(ProviderId::OpenRouter);
    assert_eq!(openrouter.runtime_id, RuntimeProviderId::OpenRouter);
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

// Covers: qwen-token-plan must resolve as OpenAI-compatible with api-key auth
// Owner: provider registry
#[test]
fn qwen_token_plan_is_openai_compatible_with_api_key_auth() {
    use super::{ProviderId, RuntimeProviderId};
    use crate::model::registry::{provider_runtime, ProviderRuntime, QWEN_TOKEN_PLAN_API_BASE};

    let descriptor = super::provider_descriptor_by_id(ProviderId::QwenTokenPlan);
    assert_eq!(descriptor.name, "qwen-token-plan");
    assert_eq!(descriptor.runtime_id, RuntimeProviderId::QwenTokenPlan);
    assert!(descriptor.auth_mode("qwen-token-plan-api-key").is_some());
    assert_eq!(
        provider_runtime("qwen-token-plan"),
        Some(ProviderRuntime::OpenAiCompatible {
            dialect: crate::providers::openai_compatible::OpenAiCompatibleDialect::Standard,
            default_api_base: QWEN_TOKEN_PLAN_API_BASE,
        })
    );
}
