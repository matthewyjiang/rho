use super::super::{CatalogReasoningPolicy, ProviderAuthKind, ProviderId, ProviderModelSource};
use super::{
    custom_openai_compatible_provider, custom_openai_compatible_providers,
    install_custom_openai_compatible_providers, validate_custom_provider_name,
};

fn restore_empty() {
    install_custom_openai_compatible_providers(std::iter::empty::<&str>()).unwrap();
}

// Covers: custom names must not collide with built-ins or break /model provider/model
// Owner: provider registry
#[test]
fn custom_provider_names_reject_reserved_and_invalid_values() {
    for name in [
        "",
        "all",
        "openai",
        "openrouter-oauth",
        "Composer",
        "vllm/local",
        "1host",
    ] {
        assert!(
            validate_custom_provider_name(name).is_err(),
            "expected {name:?} to be rejected"
        );
    }
    validate_custom_provider_name("composer").unwrap();
    validate_custom_provider_name("vllm-local").unwrap();
}

// Covers: installed custom hosts resolve as keyless OpenAI-compatible providers
// Owner: provider registry
#[test]
fn install_custom_providers_makes_keyless_openai_compatible_hosts() {
    restore_empty();
    install_custom_openai_compatible_providers(["composer", "vllm"]).unwrap();

    let composer = custom_openai_compatible_provider("composer").expect("composer");
    assert_eq!(composer.name, "composer");
    assert_eq!(composer.display_name, "composer");
    assert_eq!(composer.id, ProviderId::OpenAiCompatible);
    assert_eq!(
        composer.model_source,
        ProviderModelSource::CachedProviderModels
    );
    assert_eq!(
        composer.catalog_reasoning,
        CatalogReasoningPolicy::OffAsNone
    );
    assert!(matches!(
        composer.default_auth().auth_kind,
        ProviderAuthKind::None
    ));
    assert_eq!(
        custom_openai_compatible_providers()
            .iter()
            .map(|descriptor| descriptor.name)
            .collect::<Vec<_>>(),
        ["composer", "vllm"]
    );
    assert!(
        crate::model::catalog::login_groups()
            .iter()
            .all(|group| group.id != "composer" && group.id != "vllm"),
        "custom hosts must not appear in /login"
    );

    restore_empty();
    assert!(custom_openai_compatible_provider("composer").is_none());
}
