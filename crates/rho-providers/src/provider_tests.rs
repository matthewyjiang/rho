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
    for provider in [ProviderId::KimiCode, ProviderId::Xai] {
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

#[test]
fn provider_auth_metadata_exposes_stable_storage_and_environment_keys() {
    use super::{ProviderAuthKind, ProviderId};

    let openai = super::provider_descriptor_by_id(ProviderId::OpenAi).default_auth();
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

    let codex = super::provider_descriptor_by_id(ProviderId::OpenAiCodex).default_auth();
    assert_eq!(codex.auth_kind.env_var(), Some("CODEX_ACCESS_TOKEN"));
    assert_eq!(codex.auth_kind.account(), Some(super::CODEX_TOKENS_ACCOUNT));
    assert!(matches!(
        codex.auth_kind,
        ProviderAuthKind::CodexOAuth {
            account: super::CODEX_TOKENS_ACCOUNT,
            ..
        }
    ));

    let google = super::provider_descriptor_by_id(ProviderId::Google).default_auth();
    assert_eq!(google.id, "google-api-key");
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

    let moonshot = super::provider_descriptor_by_id(ProviderId::Moonshot).default_auth();
    assert_eq!(moonshot.id, "moonshot-api-key");
    assert_eq!(moonshot.auth_kind.env_var(), Some("MOONSHOT_API_KEY"));
    assert_eq!(
        moonshot.auth_kind.account(),
        Some(super::MOONSHOT_API_KEY_ACCOUNT)
    );

    let poolside = super::provider_descriptor_by_id(ProviderId::Poolside).default_auth();
    assert_eq!(poolside.id, "poolside-api-key");
    assert_eq!(poolside.auth_kind.env_var(), Some("POOLSIDE_API_KEY"));
    assert_eq!(
        poolside.auth_kind.account(),
        Some(super::POOLSIDE_API_KEY_ACCOUNT)
    );

    let openrouter = super::provider_descriptor_by_id(ProviderId::OpenRouter).default_auth();
    assert_eq!(openrouter.id, "openrouter-api-key");
    assert_eq!(openrouter.auth_kind.env_var(), Some("OPENROUTER_API_KEY"));
    assert_eq!(
        openrouter.auth_kind.account(),
        Some(super::OPENROUTER_API_KEY_ACCOUNT)
    );

    let openrouter_oauth = super::provider_descriptor_by_id(ProviderId::OpenRouter)
        .auth_mode("openrouter-oauth")
        .unwrap();
    assert_eq!(openrouter_oauth.id, "openrouter-oauth");
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

    let kimi = super::provider_descriptor_by_id(ProviderId::KimiCode).default_auth();
    assert_eq!(kimi.id, "kimi-oauth");
    assert_eq!(kimi.auth_kind.env_var(), Some("KIMI_ACCESS_TOKEN"));
    assert_eq!(kimi.auth_kind.account(), Some(super::KIMI_TOKENS_ACCOUNT));
    assert!(matches!(
        kimi.auth_kind,
        ProviderAuthKind::KimiOAuth {
            account: super::KIMI_TOKENS_ACCOUNT,
            ..
        }
    ));

    let xai = super::provider_descriptor_by_id(ProviderId::Xai).default_auth();
    assert_eq!(xai.id, "xai-api-key");
    assert_eq!(xai.auth_kind.env_var(), Some("XAI_API_KEY"));
    assert_eq!(xai.auth_kind.account(), Some(super::XAI_API_KEY_ACCOUNT));
    assert!(matches!(
        xai.auth_kind,
        ProviderAuthKind::ApiKey {
            account: super::XAI_API_KEY_ACCOUNT,
            ..
        }
    ));

    let xai_oauth = super::provider_descriptor_by_id(ProviderId::Xai)
        .auth_mode("xai-oauth")
        .unwrap();
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
    assert!(ollama.is_keyless());
    assert_eq!(ollama.default_auth().auth_kind, ProviderAuthKind::None);
    assert_eq!(ollama.default_auth().auth_kind.env_var(), None);
    assert_eq!(ollama.default_auth().auth_kind.account(), None);
    assert_eq!(
        ollama.model_source,
        ProviderModelSource::CachedProviderModels
    );
    assert_eq!(
        ollama.model_refresh,
        Some(ProviderModelRefreshKind::OpenAiCompatible)
    );
}

#[test]
fn auth_profiles_are_derived_from_provider_table() {
    let expected: Vec<&str> = super::providers()
        .iter()
        .flat_map(|descriptor| descriptor.auth_modes())
        .map(|mode| mode.id)
        .filter(|auth| *auth != "none")
        .collect();
    assert_eq!(super::auth_profiles(), expected.as_slice());
    assert!(expected.contains(&"ollama-cloud-api-key"));
    assert!(expected.contains(&"ollama-cloud-device"));
    assert!(!expected.contains(&"none"));
}

#[test]
fn ollama_cloud_missing_credentials_come_from_descriptor_message() {
    use super::ProviderId;

    let error = crate::model::registry::missing_credentials_error("ollama-cloud");
    // Provider-level missing credentials use the default auth mode message.
    assert!(error.to_string().contains("ollama-cloud"));
    assert!(error.to_string().contains("/login ollama-cloud"));
    assert!(error.to_string().contains("OLLAMA_API_KEY"));

    let default_message = super::provider_descriptor_by_id(ProviderId::OllamaCloud)
        .default_auth()
        .auth_kind
        .missing_message()
        .expect("ollama-cloud default mode requires credentials");
    assert_eq!(error.to_string(), default_message);
}

#[test]
fn ollama_cloud_device_auth_mode_is_registered_on_provider() {
    use super::ProviderId;

    let descriptor = super::provider_descriptor_by_id(ProviderId::OllamaCloud);
    let mode = descriptor
        .auth_mode("ollama-cloud-device")
        .expect("device auth mode");
    assert_eq!(mode.id, "ollama-cloud-device");
    assert!(mode.auth_kind.account().is_none());
    let message = mode
        .auth_kind
        .missing_message()
        .expect("device mode requires credentials");
    assert!(message.contains("/login ollama-cloud"));
    assert!(message.contains("ollama signin"));

    let profile = super::resolve_profile("ollama-cloud", "ollama-cloud-device").unwrap();
    assert_eq!(profile.provider_name(), "ollama-cloud");
    assert_eq!(profile.auth_id(), "ollama-cloud-device");
}

#[test]
fn credential_env_vars_track_provider_auth_kinds() {
    let mut expected: Vec<&str> = super::providers()
        .iter()
        .flat_map(|descriptor| descriptor.auth_modes())
        .filter_map(|mode| mode.auth_kind.env_var())
        .collect();
    expected.sort_unstable();
    expected.dedup();

    assert_eq!(super::credential_env_vars(), expected.as_slice());
    assert!(expected.contains(&"OPENAI_API_KEY"));
    assert!(expected.contains(&"ANTHROPIC_API_KEY"));
    assert!(expected.contains(&"GEMINI_API_KEY"));
    assert!(expected.contains(&"POOLSIDE_API_KEY"));
    assert!(expected.contains(&"OLLAMA_API_KEY"));
    assert!(expected.contains(&"XAI_ACCESS_TOKEN"));
    // Keyless providers must not invent env vars.
    assert!(!expected.iter().any(|name| name.is_empty()));
}
