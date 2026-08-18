use super::super::{
    CatalogReasoningPolicy, ProviderAuthKind, ProviderModelSource, ProviderRuntime,
    UnknownEffortPolicy,
};
use super::{
    custom_openai_compatible_provider, custom_openai_compatible_providers,
    custom_provider_registry_test_lock, install_custom_openai_compatible_providers,
    intern_custom_openai_compatible_providers, is_custom_provider_api_key_auth,
    reset_custom_openai_compatible_providers_for_tests, validate_custom_provider_name,
    CustomProviderSpec, CustomProviderThreadScope,
};
use crate::openai_compatible_dialect::OpenAiCompatibleDialect;
use crate::protocol::openai_chat::ChatToolCallPolicy;

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

// Covers: installed custom hosts resolve as OpenAI-compatible with optional API key
// Owner: provider registry
#[test]
fn install_custom_providers_makes_openai_compatible_hosts() {
    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers(["composer", "vllm"]).unwrap();

    let composer = custom_openai_compatible_provider("composer").expect("composer");
    assert_eq!(composer.name, "composer");
    assert_eq!(composer.display_name, "composer");
    assert_eq!(
        composer.model_source,
        ProviderModelSource::CachedProviderModels
    );
    assert_eq!(
        composer.catalog_reasoning,
        CatalogReasoningPolicy::OffAsNone
    );
    assert!(composer.is_custom_openai_compatible());
    assert!(matches!(
        composer.runtime,
        ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Custom,
            ..
        }
    ));
    assert_eq!(
        OpenAiCompatibleDialect::Custom.chat_tool_call_policy(),
        ChatToolCallPolicy::Lenient
    );
    assert!(!composer.is_keyless());
    assert!(composer.has_none_auth());
    assert_eq!(
        composer.unknown_effort(),
        UnknownEffortPolicy::SendRequested
    );
    assert!(matches!(
        composer.default_auth().auth_kind,
        ProviderAuthKind::None
    ));
    let api_key = composer
        .auth_mode("composer-api-key")
        .expect("custom hosts expose an optional API key mode");
    assert!(matches!(
        api_key.auth_kind,
        ProviderAuthKind::ApiKey { account, env_var, .. }
            if account == "provider:composer:api-key" && env_var == "RHO_COMPOSER_API_KEY"
    ));
    assert_eq!(
        crate::provider::resolve_auth_mode("composer-api-key")
            .map(|(descriptor, mode)| (descriptor.name, mode.id)),
        Some(("composer", "composer-api-key"))
    );
    let listed = crate::provider::visible_providers();
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
    let composer_group = crate::model::catalog::login_groups()
        .into_iter()
        .find(|group| group.id == "composer")
        .expect("installed custom hosts are login groups");
    assert_eq!(composer_group.methods.len(), 1);
    assert_eq!(composer_group.methods[0].target.auth, "composer-api-key");
    assert!(crate::model::catalog::login_groups()
        .iter()
        .any(|group| group.id == "vllm"));
    assert!(
        crate::model::catalog::login_targets()
            .iter()
            .any(|target| target.auth == "composer-api-key" && target.provider == "composer"),
        "installed custom hosts must be login targets for optional API keys"
    );

    restore_empty();
    assert!(custom_openai_compatible_provider("composer").is_none());
}

// Covers: a borrowed catalog slug becomes the host's models.dev upstream
// Owner: provider registry
#[test]
fn custom_host_catalog_slug_becomes_metadata_upstream() {
    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers([
        CustomProviderSpec::new("cliproxyapi", Some("llmgateway")),
        // A blank slug borrows nothing.
        CustomProviderSpec::new("blank", Some("  ")),
        CustomProviderSpec::new("vllm", None),
    ])
    .unwrap();

    let borrowed = custom_openai_compatible_provider("cliproxyapi").unwrap();
    assert_eq!(borrowed.metadata_upstream, "llmgateway");
    assert_eq!(borrowed.name, "cliproxyapi");
    assert_eq!(
        custom_openai_compatible_provider("blank")
            .unwrap()
            .metadata_upstream,
        "blank"
    );
    assert_eq!(
        custom_openai_compatible_provider("vllm")
            .unwrap()
            .metadata_upstream,
        "vllm"
    );
}

// Covers: repointing catalog must not keep serving the previously leaked slug
// Owner: provider registry
#[test]
fn custom_host_catalog_change_reinterns_the_descriptor() {
    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers([CustomProviderSpec::new(
        "cliproxyapi",
        Some("llmgateway"),
    )])
    .unwrap();
    install_custom_openai_compatible_providers([CustomProviderSpec::new(
        "cliproxyapi",
        Some("openai-codex"),
    )])
    .unwrap();

    assert_eq!(
        custom_openai_compatible_provider("cliproxyapi")
            .unwrap()
            .metadata_upstream,
        "openai-codex"
    );
}

// Covers: a later config replaces the active custom provider set
// Owner: provider registry
#[test]
fn install_custom_providers_replaces_the_active_set() {
    let _lock = custom_provider_registry_test_lock();
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

// Covers: a runtime overlay must not replace another runtime's process-wide names
// Owner: provider registry
#[test]
fn thread_scope_does_not_replace_process_active_providers() {
    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers(["composer"]).unwrap();
    let overlay = intern_custom_openai_compatible_providers(["vllm"]).unwrap();
    {
        let _scope = CustomProviderThreadScope::enter(overlay);
        assert!(crate::provider::provider_descriptor("vllm").is_some());
        assert!(crate::provider::provider_descriptor("composer").is_none());
    }
    assert!(crate::provider::provider_descriptor("composer").is_some());
    assert!(crate::provider::provider_descriptor("vllm").is_none());
}

// Covers: a host installed while no overlay is active is immediately visible
// Owner: provider registry
#[test]
fn installing_a_new_host_is_visible_without_an_overlay() {
    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers(["composer"]).unwrap();
    assert!(crate::provider::provider_descriptor("vllm").is_none());

    install_custom_openai_compatible_providers(["composer", "vllm"]).unwrap();
    assert!(crate::provider::provider_descriptor("composer").is_some());
    assert!(crate::provider::provider_descriptor("vllm").is_some());
}

#[test]
fn custom_api_key_auth_ids_accept_valid_host_names_only() {
    assert!(is_custom_provider_api_key_auth("vllm-api-key"));
    assert!(is_custom_provider_api_key_auth("composer-api-key"));
    assert!(!is_custom_provider_api_key_auth("openai-api-key"));
    assert!(!is_custom_provider_api_key_auth("api-key"));
    assert!(!is_custom_provider_api_key_auth("vllm"));
}

// Covers: model discovery must use a stored key instead of probing anonymously
// Owner: provider registry
#[test]
fn discovery_auth_prefers_a_stored_key_over_the_keyless_default() {
    use crate::credentials::{CredentialStore, MemoryCredentialStore};

    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    install_custom_openai_compatible_providers(["composer"]).unwrap();
    let descriptor = custom_openai_compatible_provider("composer").unwrap();

    let store = MemoryCredentialStore::default();
    assert_eq!(
        descriptor.discovery_auth(&store).id,
        "none",
        "a host with no stored key is probed anonymously"
    );

    store
        .set_secret("provider:composer:api-key", "composer-secret")
        .unwrap();
    assert_eq!(
        descriptor.discovery_auth(&store).id,
        "composer-api-key",
        "a stored key must be used so discovery does not 401"
    );
}

// Covers: custom-host env overrides must be stripped from child processes
// Owner: provider registry
#[test]
fn credential_env_vars_include_interned_custom_hosts() {
    let _lock = custom_provider_registry_test_lock();
    restore_empty();
    let _restore = RestoreCustomProviders;
    intern_custom_openai_compatible_providers(["envfilter-host"]).unwrap();
    assert!(
        crate::credential_env_vars()
            .iter()
            .any(|name| name == "RHO_ENVFILTER_HOST_API_KEY"),
        "interned custom hosts must appear in the child-process strip list"
    );
}
