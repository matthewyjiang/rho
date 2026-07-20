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
fn provider_auth_metadata_exposes_stable_storage_and_environment_keys() {
    use super::{ProviderAuthKind, ProviderId};

    let openai = super::provider_descriptor_by_id(ProviderId::OpenAi);
    assert_eq!(openai.auth_kind.env_var(), "OPENAI_API_KEY");
    assert_eq!(openai.auth_kind.account(), super::OPENAI_API_KEY_ACCOUNT);
    assert!(matches!(
        openai.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::OPENAI_API_KEY_ACCOUNT,
            ..
        }
    ));

    let codex = super::provider_descriptor_by_id(ProviderId::OpenAiCodex);
    assert_eq!(codex.auth_kind.env_var(), "CODEX_ACCESS_TOKEN");
    assert_eq!(codex.auth_kind.account(), super::CODEX_TOKENS_ACCOUNT);
    assert!(matches!(
        codex.auth_kind,
        ProviderAuthKind::CodexOAuth {
            account: super::CODEX_TOKENS_ACCOUNT,
            ..
        }
    ));

    let google = super::provider_descriptor_by_id(ProviderId::Google);
    assert_eq!(google.auth, "google-api-key");
    assert_eq!(google.auth_kind.env_var(), "GEMINI_API_KEY");
    assert_eq!(google.auth_kind.account(), super::GOOGLE_API_KEY_ACCOUNT);
    assert!(matches!(
        google.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::GOOGLE_API_KEY_ACCOUNT,
            ..
        }
    ));

    let moonshot = super::provider_descriptor_by_id(ProviderId::Moonshot);
    assert_eq!(moonshot.auth, "moonshot-api-key");
    assert_eq!(moonshot.auth_kind.env_var(), "MOONSHOT_API_KEY");
    assert_eq!(
        moonshot.auth_kind.account(),
        super::MOONSHOT_API_KEY_ACCOUNT
    );

    let openrouter = super::provider_descriptor_by_id(ProviderId::OpenRouter);
    assert_eq!(openrouter.auth, "openrouter-api-key");
    assert_eq!(openrouter.auth_kind.env_var(), "OPENROUTER_API_KEY");
    assert_eq!(
        openrouter.auth_kind.account(),
        super::OPENROUTER_API_KEY_ACCOUNT
    );

    let kimi = super::provider_descriptor_by_id(ProviderId::KimiCode);
    assert_eq!(kimi.auth, "kimi-oauth");
    assert_eq!(kimi.auth_kind.env_var(), "KIMI_ACCESS_TOKEN");
    assert_eq!(kimi.auth_kind.account(), super::KIMI_TOKENS_ACCOUNT);
    assert!(matches!(
        kimi.auth_kind,
        ProviderAuthKind::KimiOAuth {
            account: super::KIMI_TOKENS_ACCOUNT,
            ..
        }
    ));

    let xai = super::provider_descriptor_by_id(ProviderId::Xai);
    assert_eq!(xai.auth, "xai-api-key");
    assert_eq!(xai.auth_kind.env_var(), "XAI_API_KEY");
    assert_eq!(xai.auth_kind.account(), super::XAI_API_KEY_ACCOUNT);
    assert!(matches!(
        xai.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::XAI_API_KEY_ACCOUNT,
            ..
        }
    ));

    let xai_oauth = super::provider_descriptor_by_id(ProviderId::XaiOAuth);
    assert_eq!(xai_oauth.auth_kind.env_var(), "XAI_ACCESS_TOKEN");
    assert_eq!(xai_oauth.auth_kind.account(), super::XAI_TOKENS_ACCOUNT);
    assert!(matches!(
        xai_oauth.auth_kind,
        ProviderAuthKind::XaiOAuth {
            account: super::XAI_TOKENS_ACCOUNT,
            ..
        }
    ));
}
