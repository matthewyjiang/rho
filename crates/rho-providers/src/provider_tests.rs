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
    }
}

#[test]
fn catalog_reasoning_policies_follow_provider_control_semantics() {
    use super::{CatalogReasoningPolicy, ProviderId};

    for provider in [ProviderId::OpenAi, ProviderId::OpenAiCodex] {
        assert_eq!(
            super::provider_descriptor_by_id(provider).catalog_reasoning,
            CatalogReasoningPolicy::ExactAdvertised
        );
    }
    assert_eq!(
        super::provider_descriptor_by_id(ProviderId::OpenRouter).catalog_reasoning,
        CatalogReasoningPolicy::OffAsNone
    );
    assert_eq!(
        super::provider_descriptor_by_id(ProviderId::OpenRouterOAuth).catalog_reasoning,
        CatalogReasoningPolicy::OffAsNone
    );
    assert_eq!(
        super::provider_descriptor_by_id(ProviderId::Poolside).catalog_reasoning,
        CatalogReasoningPolicy::OffOrMax
    );
    assert_eq!(
        super::provider_descriptor_by_id(ProviderId::GithubCopilot).catalog_reasoning,
        CatalogReasoningPolicy::NotConfigurable
    );
    assert_eq!(
        super::provider_descriptor_by_id(ProviderId::Anthropic).catalog_reasoning,
        CatalogReasoningPolicy::Unknown
    );
    assert_eq!(
        super::provider_descriptor_by_id(ProviderId::Moonshot).catalog_reasoning,
        CatalogReasoningPolicy::ExactAdvertised
    );
    for provider in [ProviderId::KimiCode, ProviderId::Xai, ProviderId::XaiOAuth] {
        assert_eq!(
            super::provider_descriptor_by_id(provider).catalog_reasoning,
            CatalogReasoningPolicy::OffByAdvertisedToggle
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
fn plain_model_id_codec_leaves_ids_unchanged() {
    use super::{ModelIdCodec, ProviderId};

    let openai = super::provider_descriptor_by_id(ProviderId::OpenAi);
    assert_eq!(openai.model_id_codec, ModelIdCodec::Plain);
    assert_eq!(openai.canonicalize_model_id("gpt-5.5"), "gpt-5.5");
    assert_eq!(openai.wire_model_id("gpt-5.5"), "gpt-5.5");
}

#[test]
fn openrouter_profiles_share_runtime_policy_and_resolve_by_auth() {
    use super::{ProviderId, RuntimeProviderId};

    let api_key = super::provider_descriptor_by_id(ProviderId::OpenRouter);
    let oauth = super::provider_descriptor_by_id(ProviderId::OpenRouterOAuth);
    assert_eq!(api_key.runtime_id, RuntimeProviderId::OpenRouter);
    assert_eq!(oauth.runtime_id, api_key.runtime_id);
    assert_eq!(
        super::resolve_profile("openrouter", "openrouter-oauth").unwrap(),
        oauth
    );
    assert_eq!(
        super::resolve_profile("openrouter-oauth", "openrouter-api-key").unwrap(),
        api_key
    );
}

#[test]
fn provider_auth_metadata_exposes_stable_storage_and_environment_keys() {
    use super::{ProviderAuthKind, ProviderId};

    let openai = super::provider_descriptor_by_id(ProviderId::OpenAi);
    assert_eq!(openai.auth_kind.env_var(), Some("OPENAI_API_KEY"));
    assert_eq!(
        openai.auth_kind.account(),
        Some(super::OPENAI_API_KEY_ACCOUNT)
    );
    assert!(matches!(
        openai.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::OPENAI_API_KEY_ACCOUNT,
            ..
        }
    ));

    let codex = super::provider_descriptor_by_id(ProviderId::OpenAiCodex);
    assert_eq!(codex.auth_kind.env_var(), Some("CODEX_ACCESS_TOKEN"));
    assert_eq!(codex.auth_kind.account(), Some(super::CODEX_TOKENS_ACCOUNT));
    assert!(matches!(
        codex.auth_kind,
        ProviderAuthKind::CodexOAuth {
            account: super::CODEX_TOKENS_ACCOUNT,
            ..
        }
    ));

    let google = super::provider_descriptor_by_id(ProviderId::Google);
    assert_eq!(google.auth, "google-api-key");
    assert_eq!(google.auth_kind.env_var(), Some("GEMINI_API_KEY"));
    assert_eq!(
        google.auth_kind.account(),
        Some(super::GOOGLE_API_KEY_ACCOUNT)
    );
    assert!(matches!(
        google.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::GOOGLE_API_KEY_ACCOUNT,
            ..
        }
    ));

    let moonshot = super::provider_descriptor_by_id(ProviderId::Moonshot);
    assert_eq!(moonshot.auth, "moonshot-api-key");
    assert_eq!(moonshot.auth_kind.env_var(), Some("MOONSHOT_API_KEY"));
    assert_eq!(
        moonshot.auth_kind.account(),
        Some(super::MOONSHOT_API_KEY_ACCOUNT)
    );

    let poolside = super::provider_descriptor_by_id(ProviderId::Poolside);
    assert_eq!(poolside.auth, "poolside-api-key");
    assert_eq!(poolside.auth_kind.env_var(), Some("POOLSIDE_API_KEY"));
    assert_eq!(
        poolside.auth_kind.account(),
        Some(super::POOLSIDE_API_KEY_ACCOUNT)
    );

    let openrouter = super::provider_descriptor_by_id(ProviderId::OpenRouter);
    assert_eq!(openrouter.auth, "openrouter-api-key");
    assert_eq!(openrouter.auth_kind.env_var(), Some("OPENROUTER_API_KEY"));
    assert_eq!(
        openrouter.auth_kind.account(),
        Some(super::OPENROUTER_API_KEY_ACCOUNT)
    );

    let openrouter_oauth = super::provider_descriptor_by_id(ProviderId::OpenRouterOAuth);
    assert_eq!(openrouter_oauth.auth, "openrouter-oauth");
    assert_eq!(
        openrouter_oauth.auth_kind.env_var(),
        Some("OPENROUTER_API_KEY")
    );
    assert_eq!(
        openrouter_oauth.auth_kind.account(),
        Some(super::OPENROUTER_OAUTH_KEY_ACCOUNT)
    );
    assert!(matches!(
        openrouter_oauth.auth_kind,
        ProviderAuthKind::BearerCredential {
            account: super::OPENROUTER_OAUTH_KEY_ACCOUNT,
            ..
        }
    ));

    let kimi = super::provider_descriptor_by_id(ProviderId::KimiCode);
    assert_eq!(kimi.auth, "kimi-oauth");
    assert_eq!(kimi.auth_kind.env_var(), Some("KIMI_ACCESS_TOKEN"));
    assert_eq!(kimi.auth_kind.account(), Some(super::KIMI_TOKENS_ACCOUNT));
    assert!(matches!(
        kimi.auth_kind,
        ProviderAuthKind::KimiOAuth {
            account: super::KIMI_TOKENS_ACCOUNT,
            ..
        }
    ));

    let xai = super::provider_descriptor_by_id(ProviderId::Xai);
    assert_eq!(xai.auth, "xai-api-key");
    assert_eq!(xai.auth_kind.env_var(), Some("XAI_API_KEY"));
    assert_eq!(xai.auth_kind.account(), Some(super::XAI_API_KEY_ACCOUNT));
    assert!(matches!(
        xai.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::XAI_API_KEY_ACCOUNT,
            ..
        }
    ));

    let xai_oauth = super::provider_descriptor_by_id(ProviderId::XaiOAuth);
    assert_eq!(xai_oauth.auth_kind.env_var(), Some("XAI_ACCESS_TOKEN"));
    assert_eq!(
        xai_oauth.auth_kind.account(),
        Some(super::XAI_TOKENS_ACCOUNT)
    );
    assert!(matches!(
        xai_oauth.auth_kind,
        ProviderAuthKind::XaiOAuth {
            account: super::XAI_TOKENS_ACCOUNT,
            ..
        }
    ));
}

#[test]
fn ollama_descriptor_is_keyless_and_refreshes_compatible_models() {
    use super::{ProviderAuthKind, ProviderId, ProviderModelRefreshKind, ProviderModelSource};

    let ollama = super::provider_descriptor_by_id(ProviderId::Ollama);
    assert_eq!(ollama.name, "ollama");
    assert_eq!(ollama.display_name, "Ollama");
    assert_eq!(ollama.auth_kind, ProviderAuthKind::None);
    assert_eq!(ollama.auth_kind.env_var(), None);
    assert_eq!(ollama.auth_kind.account(), None);
    assert_eq!(
        ollama.model_source,
        ProviderModelSource::CachedProviderModels
    );
    assert_eq!(
        ollama.model_refresh,
        Some(ProviderModelRefreshKind::OpenAiCompatible)
    );
}
