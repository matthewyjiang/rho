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
}
