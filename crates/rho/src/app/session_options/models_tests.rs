use pretty_assertions::assert_eq;
use rho_protocol as wire;
use rho_providers::model::{
    catalog::{ModelCatalogEntry, ModelSelection},
    display_name::ModelDisplayNameCacheGuard,
    models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    },
    provider_models::{
        replace_cached_aliased_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
    },
};

use super::{model_options, resolve_model, AuthPick, CurrentModel, ModelRefused, OfferedModels};

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

// Covers: a current model set by an alias id (Mistral's default
// `mistral-medium-latest`) is offered once, in its listed row's place under
// that row's label, so a host picker marks the row and shows no second one;
// a request naming the listed id still finds that row (#1945)
// Owner: session options
#[test]
fn a_current_alias_takes_its_listed_rows_place() {
    let (options, lookups) = with_catalog_names(&[], || {
        replace_cached_aliased_provider_models_for_tests(
            "mistral",
            &[(
                "mistral-small-2603",
                "Mistral Small 4",
                &["mistral-small-latest"],
            )],
        )
        .unwrap();
        let options = model_options(
            &current("mistral", "mistral-small-latest"),
            &[],
            vec![
                entry("mistral", "mistral-small-2603"),
                entry("xai", "grok-3"),
            ],
        );
        let offered = OfferedModels::new(&options);
        let found = [
            "mistral/mistral-small-2603",
            "mistral/mistral-small-latest",
            "xai/grok-3",
            "xai/mistral-small-2603",
        ]
        .map(|reference| {
            offered
                .by_reference(reference)
                .map(|choice| choice.model.clone())
        });
        let by_id = options
            .available
            .iter()
            .filter(|choice| offered.answers(choice, "mistral-small-2603"))
            .count();
        (options, (found, by_id))
    });

    assert_eq!(
        options,
        wire::ModelOptions {
            current: model_ref("mistral", "mistral-small-latest"),
            auth: "api-key".into(),
            available: vec![
                choice("mistral/mistral-small-latest", "Mistral Small 4"),
                choice("xai/grok-3", "grok-3"),
            ],
        }
    );
    assert_eq!(
        lookups,
        (
            [
                Some(model_ref("mistral", "mistral-small-latest")),
                Some(model_ref("mistral", "mistral-small-latest")),
                Some(model_ref("xai", "grok-3")),
                None,
            ],
            1
        )
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
        resolve_model(
            provider,
            model,
            &current("xai", "grok-3"),
            AuthPick::AnyOf(&available_auths),
        )
    });
    let kept = resolve_model(
        "custom",
        "local-model",
        &current("custom", "local-model"),
        AuthPick::AnyOf(&available_auths),
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

// Covers: a pick that names its auth runs on that auth, for another model
// and for the current one, and is refused when the model's provider does
// not take it, rather than run on the provider's default.
// Owner: session options
#[test]
fn a_pick_that_names_its_auth_runs_on_it_or_is_refused() {
    let on_key = CurrentModel {
        provider: "xai".into(),
        model: "grok-4.7".into(),
        auth: "xai-api-key".into(),
    };
    let picks = [
        ("xai", "grok-4.6", "xai-oauth"),
        ("xai", "grok-4.7", "xai-oauth"),
        ("xai", "grok-4.7", "xai-api-key"),
        ("xai", "grok-4.7", "codex"),
    ]
    .map(|(provider, model, auth)| {
        resolve_model(provider, model, &on_key, AuthPick::Named(auth))
            .map(|selection| selection.auth)
    });

    assert_eq!(
        picks,
        [
            Ok("xai-oauth".to_owned()),
            Ok("xai-oauth".to_owned()),
            Ok("xai-api-key".to_owned()),
            Err(ModelRefused(
                "model 'xai/grok-4.7' does not run on auth 'codex'".into()
            )),
        ]
    );
}
