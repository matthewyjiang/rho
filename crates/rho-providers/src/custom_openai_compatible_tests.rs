use super::super::{
    CatalogReasoningPolicy, ProviderAuthKind, ProviderId, ProviderModelSource, UnknownEffortPolicy,
};
use super::{
    custom_openai_compatible_provider, custom_openai_compatible_providers,
    install_custom_openai_compatible_providers, reset_custom_openai_compatible_providers_for_tests,
    validate_custom_provider_name,
};

fn restore_empty() {
    reset_custom_openai_compatible_providers_for_tests();
}

struct RestoreCustomProviders;

impl Drop for RestoreCustomProviders {
    fn drop(&mut self) {
        restore_empty();
    }
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
    let _restore = RestoreCustomProviders;
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
    assert_eq!(composer.unknown_effort, UnknownEffortPolicy::SendRequested);
    assert!(composer.is_custom_openai_compatible());
    assert!(matches!(
        composer.default_auth().auth_kind,
        ProviderAuthKind::None
    ));
    let listed = crate::provider::providers();
    assert!(
        listed
            .iter()
            .any(|descriptor| descriptor.name == "composer")
            && listed.iter().any(|descriptor| descriptor.name == "vllm"),
        "each custom name must appear as its own provider"
    );
    assert_eq!(
        crate::provider::provider_descriptor("composer").map(|descriptor| descriptor.name),
        Some("composer")
    );
    assert_eq!(
        crate::provider::provider_descriptor("vllm").map(|descriptor| descriptor.name),
        Some("vllm")
    );
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

// Covers: a later config replaces the active custom provider set
// Owner: provider registry
#[test]
fn install_custom_providers_replaces_the_active_set() {
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers(["composer"]).unwrap();
    install_custom_openai_compatible_providers(["vllm"]).unwrap();

    assert!(custom_openai_compatible_provider("composer").is_none());
    assert!(custom_openai_compatible_provider("vllm").is_some());
    assert_eq!(
        custom_openai_compatible_providers()
            .iter()
            .map(|descriptor| descriptor.name)
            .collect::<Vec<_>>(),
        ["vllm"]
    );
    assert!(crate::provider::provider_descriptor("composer").is_none());
    assert!(crate::provider::provider_descriptor("vllm").is_some());
}
