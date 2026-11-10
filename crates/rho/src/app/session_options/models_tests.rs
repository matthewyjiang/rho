use pretty_assertions::assert_eq;
use rho_protocol as wire;
use rho_providers::model::{
    catalog::{ModelCatalogEntry, ModelSelection},
    display_name::ModelDisplayNameCacheGuard,
    models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    },
    provider_models::with_provider_models_cache_dir_for_tests,
};

use super::{model_options, resolve_model, CurrentModel, ModelRefused};

fn entry(provider: &str, model: &str) -> ModelCatalogEntry {
    ModelCatalogEntry {
        provider: provider.into(),
        model: model.into(),
        display_name: model.into(),
        auth_modes: vec!["api-key".into()],
    }
}

fn current(provider: &str, model: &str) -> CurrentModel {
    CurrentModel {
        provider: provider.into(),
        model: model.into(),
        auth: "api-key".into(),
    }
}

fn model_ref(provider: &str, model: &str) -> wire::ModelRef {
    wire::ModelRef {
        provider: provider.into(),
        model: model.into(),
    }
}

/// A choice of `reference` (`provider/model`) labeled `label`.
fn choice(reference: &str, label: &str) -> wire::ModelChoice {
    let (provider, model) = reference.split_once('/').expect("provider/model");
    wire::ModelChoice {
        model: model_ref(provider, model),
        label: label.into(),
    }
}

/// Runs `f` against empty catalog caches holding only the given catalog names,
/// so labels never read the developer's real models.dev snapshot.
fn with_catalog_names<T>(names: &[(&str, &str, &str)], f: impl FnOnce() -> T) -> T {
    let catalog = tempfile::tempdir().unwrap();
    let provider = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
        with_provider_models_cache_dir_for_tests(provider.path().to_path_buf(), || {
            let _names = ModelDisplayNameCacheGuard::new();
            for (provider_name, model, display_name) in names {
                let mut metadata = ModelMetadata::default();
                metadata.display_name = Some((*display_name).into());
                write_cached_model_metadata_for_tests(provider_name, model, &metadata);
            }
            f()
        })
    })
}

// Covers: model choices must follow TUI picker order and use short labels
// that host pickers do not truncate: the catalog name, else the bare model
// id, with the provider added only when two rows would otherwise share a
// label, and the model id when the same provider names two models alike
// Owner: session options
#[test]
fn model_options_list_favorites_first_with_short_unique_labels() {
    let names = [
        ("xai", "grok-3", "Grok 3"),
        ("openai", "gpt-5.5", "GPT-5.5"),
        ("openai-codex", "gpt-5.5", "GPT-5.5"),
        ("openai", "gpt-5.6-sol", "GPT-5.6 Sol"),
        ("openai", "gpt-5.6-sol-pro", "GPT-5.6 Sol"),
        ("xai", "grok-a", "Grok"),
        ("xai", "grok-b", "Grok"),
        ("opencode-go", "grok", "Grok"),
    ];
    let options = with_catalog_names(&names, || {
        model_options(
            &current("openai", "gpt-4"),
            &["xai/grok-3".into()],
            vec![
                entry("openai", "gpt-4"),
                entry("openai", "gpt-5.5"),
                entry("openai-codex", "gpt-5.5"),
                entry("xai", "grok-3"),
                entry("openai", "gpt-5.6-sol"),
                entry("openai", "gpt-5.6-sol-pro"),
                entry("xai", "grok-a"),
                entry("xai", "grok-b"),
                entry("opencode-go", "grok"),
            ],
        )
    });

    let expected = wire::ModelOptions {
        current: model_ref("openai", "gpt-4"),
        auth: "api-key".into(),
        available: vec![
            choice("xai/grok-3", "Grok 3"),
            choice("openai/gpt-4", "gpt-4"),
            choice("openai/gpt-5.5", "GPT-5.5 (openai)"),
            choice("openai-codex/gpt-5.5", "GPT-5.5 (openai-codex)"),
            choice("openai/gpt-5.6-sol", "gpt-5.6-sol"),
            choice("openai/gpt-5.6-sol-pro", "gpt-5.6-sol-pro"),
            choice("xai/grok-a", "grok-a (xai)"),
            choice("xai/grok-b", "grok-b (xai)"),
            choice("opencode-go/grok", "Grok (opencode-go)"),
        ],
    };
    assert_eq!(options, expected);
}

// Covers: the current model must remain selectable even when it is off-catalog
// Owner: session options
#[test]
fn current_model_absent_from_available_is_appended() {
    let options = with_catalog_names(&[], || {
        model_options(
            &current("custom", "local-model"),
            &[],
            vec![entry("xai", "grok-3")],
        )
    });

    assert_eq!(
        options,
        wire::ModelOptions {
            current: model_ref("custom", "local-model"),
            auth: "api-key".into(),
            available: vec![
                choice("xai/grok-3", "grok-3"),
                choice("custom/local-model", "local-model"),
            ],
        }
    );
}

// Covers: a pick no catalog serves is refused with a message naming it, and
// the current model, even off-catalog, resolves to itself with its auth
// Owner: session options
#[test]
fn resolve_model_keeps_the_current_model_and_refuses_unknown_ones() {
    let available_auths = ["xai-api-key".to_string()];
    let refused = [
        ("no-such-provider", "no-such-model"),
        ("xai", "no-such-model"),
    ]
    .map(|(provider, model)| {
        resolve_model(provider, model, &current("xai", "grok-3"), &available_auths)
    });
    let kept = resolve_model(
        "custom",
        "local-model",
        &current("custom", "local-model"),
        &available_auths,
    );

    assert_eq!(
        refused,
        [
            Err(ModelRefused(
                "unknown provider 'no-such-provider' for model value \
                 'no-such-provider/no-such-model'"
                    .into()
            )),
            Err(ModelRefused(
                "model 'no-such-model' is not available for provider 'xai' (value \
                 'xai/no-such-model'). Choose a model from the provider allowlist."
                    .into()
            )),
        ]
    );
    assert_eq!(
        kept,
        Ok(ModelSelection {
            provider: "custom".into(),
            model: "local-model".into(),
            auth: "api-key".into(),
            from_catalog: false,
        })
    );
}
