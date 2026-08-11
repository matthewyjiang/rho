use pretty_assertions::assert_eq;

use super::*;
use crate::model::{
    models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    },
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ReasoningCapabilities,
};

/// Runs `f` against empty models.dev and provider-model caches.
fn with_empty_caches<T>(f: impl FnOnce() -> T) -> T {
    let catalog = tempfile::tempdir().unwrap();
    let provider = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
        with_provider_models_cache_dir_for_tests(provider.path().to_path_buf(), || {
            // Names resolve once per process; each case needs a fresh read.
            clear_model_display_name_cache_for_tests();
            f()
        })
    })
}

fn named_metadata(display_name: &str) -> ModelMetadata {
    ModelMetadata {
        display_name: Some(display_name.to_string()),
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    }
}

fn provider_model(model: &str, display_name: &str) -> ProviderModel {
    ProviderModel {
        provider: "anthropic".into(),
        model: model.into(),
        display_name: display_name.into(),
        context_window: None,
        max_output_tokens: None,
        reasoning_capabilities: ReasoningCapabilities::Unknown,
    }
}

/// Covers: a name that lands mid-session must reach the next text that names
/// the model. Pinning the first miss for the process would strand the startup
/// prefetch, which cannot beat the system prompt to the first lookup.
/// Owner: pure unit
#[test]
fn a_catalog_write_reaches_a_lookup_that_already_missed() {
    let writers: [(&str, fn()); 2] = [
        ("models.dev row", || {
            write_cached_model_metadata_for_tests(
                "anthropic",
                "claude-fable-5",
                &named_metadata("Claude Fable 5"),
            );
        }),
        ("provider model list", || {
            replace_cached_provider_models_for_tests(
                "anthropic",
                &[provider_model("claude-fable-5", "Claude Fable 5")],
            )
            .unwrap();
        }),
    ];

    for (writer, write) in writers {
        with_empty_caches(|| {
            assert_eq!(
                model_display_name("anthropic", "claude-fable-5"),
                None,
                "{writer}: no name before the write"
            );

            write();

            assert_eq!(
                model_display_name("anthropic", "claude-fable-5").as_deref(),
                Some("Claude Fable 5"),
                "{writer}: name after the write"
            );
        });
    }
}

#[test]
fn prefers_the_catalog_name_then_the_provider_name_then_nothing() {
    struct Case {
        name: &'static str,
        catalog: Option<ModelMetadata>,
        provider_models: Vec<ProviderModel>,
        expected_name: Option<&'static str>,
        expected_reference: &'static str,
    }

    let cases = [
        Case {
            name: "catalog name wins over the provider name",
            catalog: Some(named_metadata("Claude Fable 5")),
            provider_models: vec![provider_model("claude-fable-5", "Claude Fable 5 (latest)")],
            expected_name: Some("Claude Fable 5"),
            expected_reference: "anthropic/claude-fable-5 (Claude Fable 5)",
        },
        Case {
            name: "provider name fills in when the catalog has none",
            catalog: None,
            provider_models: vec![provider_model("claude-fable-5", "Claude Fable 5")],
            expected_name: Some("Claude Fable 5"),
            expected_reference: "anthropic/claude-fable-5 (Claude Fable 5)",
        },
        Case {
            name: "a provider name equal to the id is not a name",
            catalog: None,
            provider_models: vec![provider_model("claude-fable-5", "claude-fable-5")],
            expected_name: None,
            expected_reference: "anthropic/claude-fable-5",
        },
        Case {
            name: "an unknown model shows its id alone",
            catalog: None,
            provider_models: Vec::new(),
            expected_name: None,
            expected_reference: "anthropic/claude-fable-5",
        },
    ];

    for case in cases {
        with_empty_caches(|| {
            if let Some(metadata) = &case.catalog {
                write_cached_model_metadata_for_tests("anthropic", "claude-fable-5", metadata);
            }
            replace_cached_provider_models_for_tests("anthropic", &case.provider_models).unwrap();

            assert_eq!(
                model_display_name("anthropic", "claude-fable-5").as_deref(),
                case.expected_name,
                "{}",
                case.name
            );
            assert_eq!(
                model_reference_with_display_name("anthropic", "claude-fable-5"),
                case.expected_reference,
                "{}",
                case.name
            );
        });
    }
}

// Covers: ensure must not hit the network when a name is already known, or when
// the catalog has already described the model without one. Call sites that wait
// on ensure before rewriting-never text rely on that short-circuit.
// Owner: pure unit
#[tokio::test]
async fn ensure_is_a_no_op_when_a_name_is_settled() {
    struct Case {
        name: &'static str,
        catalog: Option<ModelMetadata>,
        target: (&'static str, &'static str),
    }

    let cases = [
        Case {
            name: "already has a catalog name",
            catalog: Some(named_metadata("Claude Fable 5")),
            target: ("anthropic", "claude-fable-5"),
        },
        Case {
            name: "complete row with no name is a settled miss",
            catalog: Some(ModelMetadata {
                display_name: None,
                reasoning_metadata_complete: true,
                reasoning_capabilities_known: true,
                ..ModelMetadata::default()
            }),
            target: ("anthropic", "claude-fable-5"),
        },
    ];

    for case in cases {
        let catalog = tempfile::tempdir().unwrap();
        let provider = tempfile::tempdir().unwrap();
        let written = with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
            with_provider_models_cache_dir_for_tests(provider.path().to_path_buf(), || {
                clear_model_display_name_cache_for_tests();
                if let Some(metadata) = &case.catalog {
                    write_cached_model_metadata_for_tests(case.target.0, case.target.1, metadata);
                }
                futures_util::future::FutureExt::now_or_never(ensure_model_catalog_names([(
                    case.target.0.to_string(),
                    case.target.1.to_string(),
                )]))
            })
        });
        assert_eq!(
            written,
            Some(0),
            "{}: ensure must finish without awaiting the network",
            case.name
        );
    }
}
