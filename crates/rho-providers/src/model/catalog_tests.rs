use super::*;

use pretty_assertions::assert_eq;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ReasoningCapabilities,
};

fn test_catalog() -> Vec<ModelCatalogEntry> {
    vec![
        ModelCatalogEntry {
            provider: "openai".into(),
            model: "shared-model".into(),
            display_name: "shared-model".into(),
            auth_modes: vec!["api-key".into(), "codex".into()],
        },
        ModelCatalogEntry {
            provider: "openai".into(),
            model: "unique-openai".into(),
            display_name: "unique-openai".into(),
            auth_modes: vec!["api-key".into()],
        },
        ModelCatalogEntry {
            provider: "openai".into(),
            model: "shared-model".into(),
            display_name: "shared-model duplicate".into(),
            auth_modes: vec!["api-key".into()],
        },
        ModelCatalogEntry {
            provider: "openai-codex".into(),
            model: "unique-codex".into(),
            display_name: "unique-codex".into(),
            auth_modes: vec!["codex".into()],
        },
        ModelCatalogEntry {
            provider: "anthropic".into(),
            model: "unique-anthropic".into(),
            display_name: "unique-anthropic".into(),
            auth_modes: vec!["anthropic-api-key".into()],
        },
        ModelCatalogEntry {
            provider: "future".into(),
            model: "future-model".into(),
            display_name: "future-model".into(),
            auth_modes: vec!["api-key".into()],
        },
    ]
}

fn unique_cache_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock should be after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("rho-catalog-{name}-{}-{nanos}", std::process::id()))
}

fn with_cached_provider_models<T>(
    provider: &str,
    models: Vec<ProviderModel>,
    f: impl FnOnce() -> T,
) -> T {
    let cache_dir = unique_cache_dir(provider);
    let result = with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(provider, &models).unwrap();
        f()
    });
    let _ = std::fs::remove_dir_all(cache_dir);
    result
}

fn provider_model(provider: &str, model: &str) -> ProviderModel {
    ProviderModel {
        provider: provider.into(),
        model: model.into(),
        display_name: model.into(),
        context_window: None,
        max_output_tokens: None,
        reasoning_capabilities: ReasoningCapabilities::Unknown,
    }
}

#[test]
fn resolves_poolside_references_to_internal_model_id() {
    with_cached_provider_models(
        "poolside",
        vec![provider_model("poolside", "laguna-m.1")],
        || {
            let clean = resolve_model_selection_for_provider("poolside", "laguna-m.1").unwrap();
            let legacy =
                resolve_model_selection_for_provider("poolside", "poolside/laguna-m.1").unwrap();
            let double =
                resolve_model_selection_for_provider("poolside", "poolside/poolside/laguna-m.1")
                    .unwrap();

            assert_eq!(clean.model, "laguna-m.1");
            assert_eq!(legacy, clean);
            assert_eq!(double, clean);
        },
    );
}

#[test]
fn parses_embedded_model_catalog() {
    let catalog = model_catalog();

    assert!(catalog
        .iter()
        .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.3-codex-spark"));
    assert!(catalog
        .iter()
        .any(|entry| entry.provider == "xai" && entry.model == "grok-4.5"));
    assert!(catalog
        .iter()
        .any(|entry| entry.provider == "xai" && entry.model == "grok-build-0.1"));
    assert!(catalog
        .iter()
        .any(|entry| entry.provider == "xai" && entry.model == "grok-composer-2.5-fast"));
    assert!(catalog
        .iter()
        .any(|entry| entry.provider == "xai" && entry.model == "grok-4.3"));
    assert!(!catalog
        .iter()
        .any(|entry| entry.provider == "github-copilot"));
}

#[test]
fn available_models_includes_xai_static_catalog() {
    let models = available_models_for_auths(&["xai-oauth".into()]);

    assert!(models.iter().all(|entry| entry.provider == "xai-oauth"));
    assert_eq!(
        models
            .iter()
            .map(|entry| entry.model.as_str())
            .collect::<Vec<_>>(),
        vec![
            "grok-4.3",
            "grok-4.5",
            "grok-build-0.1",
            "grok-composer-2.5-fast",
        ]
    );
    assert_eq!(
        default_model_for_provider("xai").as_deref(),
        Some("grok-4.5")
    );
}

#[test]
fn resolves_xai_static_catalog_selection() {
    let selection = resolve_model_selection_for_auths(
        "xai-oauth/grok-4.5",
        "openai",
        "api-key",
        &["xai-oauth".into()],
    )
    .unwrap();

    assert_eq!(
        selection,
        ModelSelection {
            provider: "xai-oauth".into(),
            model: "grok-4.5".into(),
            auth: "xai-oauth".into(),
            from_catalog: true,
        }
    );
}

#[test]
fn available_models_filters_to_implemented_providers() {
    let models = available_models_for_auths(&["codex".into()]);

    assert!(models.iter().all(|entry| entry.provider == "openai-codex"));
    assert!(models
        .iter()
        .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.5"));
    assert!(models
        .iter()
        .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.4-mini"));
    assert!(models
        .iter()
        .any(|entry| entry.provider == "openai-codex" && entry.model == "gpt-5.3-codex-spark"));
    assert!(models
        .iter()
        .all(|entry| implemented_providers().contains(&entry.provider.as_str())));
}

#[test]
fn available_models_for_auths_uses_static_catalog_for_subscription_models() {
    let models = available_models_for_auths(&["api-key".into(), "codex".into()]);

    assert!(models.iter().any(|entry| entry.provider == "openai-codex"));
}

#[test]
fn login_groups_are_alphabetical_by_prompt() {
    let prompts = login_groups()
        .into_iter()
        .map(|group| group.prompt)
        .collect::<Vec<_>>();
    assert!(prompts
        .windows(2)
        .all(|pair| { pair[0].to_ascii_lowercase() <= pair[1].to_ascii_lowercase() }));
}

#[test]
fn login_targets_use_provider_names() {
    let targets = login_targets();

    let providers = targets
        .iter()
        .map(|target| (target.provider.as_str(), target.auth.as_str()))
        .collect::<Vec<_>>();
    assert!(providers.contains(&("openai", "api-key")));
    assert!(providers.contains(&("openai-codex", "codex")));
    assert!(providers.contains(&("anthropic", "anthropic-api-key")));
    assert!(providers.contains(&("google", "google-api-key")));
    assert!(providers.contains(&("github-copilot", "github-copilot")));
    assert!(providers.contains(&("moonshot", "moonshot-api-key")));
    assert!(providers.contains(&("openrouter", "openrouter-api-key")));
    assert!(providers.contains(&("openrouter-oauth", "openrouter-oauth")));
    assert!(providers.contains(&("kimi-code", "kimi-oauth")));
    assert!(providers.contains(&("xai", "xai-api-key")));
    assert!(providers.contains(&("xai-oauth", "xai-oauth")));
    let google = login_group("google").expect("Google login group");
    assert_eq!(google.methods.len(), 1);
    assert_eq!(google.methods[0].target.provider, "google");
    assert!(login_target_for_provider("ollama").is_none());
    assert!(login_target_for_provider("api-key").is_none());
    assert!(login_target_for_provider("codex").is_none());
    assert!(login_target_for_provider("anthropic-api-key").is_none());
    assert!(login_target_for_provider("xai-api-key").is_none());
    assert!(login_target_for_provider("xai-oauth").is_some());
}

#[test]
fn github_copilot_requires_cached_models() {
    let cache_dir = unique_cache_dir("github-copilot-empty");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        assert_eq!(default_model_for_provider("github-copilot"), None);
        let err = resolve_model_selection_for_auths(
            "github-copilot/gpt-4.1",
            "openai",
            "api-key",
            &["github-copilot".into()],
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelSelectionError::UnavailableModel {
                provider: "github-copilot".into(),
                model: "gpt-4.1".into(),
                hint: "Open /config and choose Refresh model lists to update available models.",
            }
        );
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn github_copilot_uses_cached_models_in_picker() {
    with_cached_provider_models(
        "github-copilot",
        vec![provider_model("github-copilot", "cached-copilot-model")],
        || {
            let models = available_models_for_auths(&["github-copilot".into()]);
            assert!(models
                .iter()
                .any(|entry| entry.model == "cached-copilot-model"));
        },
    );
}

#[test]
fn resolves_provider_model_selection() {
    with_cached_provider_models("openai", vec![provider_model("openai", "gpt-5.5")], || {
        let selection = resolve_model_selection_for_auths(
            "openai/gpt-5.5",
            "openai",
            "codex",
            &["codex".into()],
        )
        .unwrap();

        assert_eq!(
            selection,
            ModelSelection {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                auth: "api-key".into(),
                from_catalog: true,
            }
        );
    });
}

#[test]
fn resolves_anthropic_provider_model_selection() {
    with_cached_provider_models(
        "anthropic",
        vec![provider_model("anthropic", "claude-sonnet-4-5")],
        || {
            let selection = resolve_model_selection_for_auths(
                "anthropic/claude-sonnet-4-5",
                "openai",
                "api-key",
                &["anthropic-api-key".into()],
            )
            .unwrap();

            assert_eq!(
                selection,
                ModelSelection {
                    provider: "anthropic".into(),
                    model: "claude-sonnet-4-5".into(),
                    auth: "anthropic-api-key".into(),
                    from_catalog: true,
                }
            );
        },
    );
}

#[test]
fn resolves_bare_cached_api_model_to_provider() {
    with_cached_provider_models(
        "openai",
        vec![provider_model("openai", "unique-openai")],
        || {
            let catalog = test_catalog();
            let selection = resolve_model_selection_from(
                &catalog,
                "unique-openai",
                "openai",
                "api-key",
                &["api-key".into()],
            )
            .unwrap();

            assert_eq!(
                selection,
                ModelSelection {
                    provider: "openai".into(),
                    model: "unique-openai".into(),
                    auth: "api-key".into(),
                    from_catalog: true,
                }
            );
        },
    );
}

#[test]
fn resolves_bare_model_across_all_available_auths() {
    let catalog = test_catalog();
    let selection = resolve_model_selection_from(
        &catalog,
        "unique-codex",
        "openai",
        "api-key",
        &["api-key".into(), "codex".into()],
    )
    .unwrap();

    assert_eq!(
        selection,
        ModelSelection {
            provider: "openai-codex".into(),
            model: "unique-codex".into(),
            auth: "codex".into(),
            from_catalog: true,
        }
    );
}

#[test]
fn resolves_bare_unique_anthropic_model() {
    with_cached_provider_models(
        "anthropic",
        vec![provider_model("anthropic", "unique-anthropic")],
        || {
            let catalog = test_catalog();
            let selection = resolve_model_selection_from(
                &catalog,
                "unique-anthropic",
                "openai",
                "api-key",
                &["api-key".into(), "anthropic-api-key".into()],
            )
            .unwrap();

            assert_eq!(
                selection,
                ModelSelection {
                    provider: "anthropic".into(),
                    model: "unique-anthropic".into(),
                    auth: "anthropic-api-key".into(),
                    from_catalog: true,
                }
            );
        },
    );
}

#[test]
fn resolves_bare_unique_codex_model() {
    let selection = resolve_model_selection_for_auths(
        "gpt-5.3-codex-spark",
        "openai-codex",
        "codex",
        &["codex".into()],
    )
    .unwrap();

    assert_eq!(
        selection,
        ModelSelection {
            provider: "openai-codex".into(),
            model: "gpt-5.3-codex-spark".into(),
            auth: "codex".into(),
            from_catalog: true,
        }
    );
}

#[test]
fn anthropic_uncached_provider_model_is_rejected() {
    let err = resolve_model_selection_for_auths(
        "anthropic/custom-model",
        "openai",
        "api-key",
        &["api-key".into()],
    )
    .unwrap_err();

    assert_eq!(
        err,
        ModelSelectionError::UnavailableModel {
            provider: "anthropic".into(),
            model: "custom-model".into(),
            hint: "Open /config and choose Refresh model lists to update available models.",
        }
    );
}

#[test]
fn bare_uncached_current_provider_model_is_rejected() {
    let err =
        resolve_model_selection_for_auths("brand-new-model", "openai", "codex", &["codex".into()])
            .unwrap_err();

    assert_eq!(
        err,
        ModelSelectionError::UnavailableModel {
            provider: "openai".into(),
            model: "brand-new-model".into(),
            hint: "Open /config and choose Refresh model lists to update available models.",
        }
    );
}

#[test]
fn bare_ambiguous_model_returns_error() {
    with_cached_provider_models(
        "openai",
        vec![provider_model("openai", "shared-model")],
        || {
            let catalog = vec![ModelCatalogEntry {
                provider: "openai-codex".into(),
                model: "shared-model".into(),
                display_name: "shared-model".into(),
                auth_modes: vec!["codex".into()],
            }];
            let err = resolve_model_selection_from(
                &catalog,
                "shared-model",
                "openai",
                "api-key",
                &["api-key".into(), "codex".into()],
            )
            .unwrap_err();

            assert_eq!(
                err,
                ModelSelectionError::AmbiguousModel {
                    model: "shared-model".into()
                }
            );
        },
    );
}

#[test]
fn non_allowlisted_codex_model_is_rejected() {
    let err = resolve_model_selection_for_auths(
        "openai-codex/custom-model",
        "openai-codex",
        "codex",
        &["codex".into()],
    )
    .unwrap_err();

    assert_eq!(
        err,
        ModelSelectionError::UnavailableModel {
            provider: "openai-codex".into(),
            model: "custom-model".into(),
            hint: "Choose a model from the provider allowlist.",
        }
    );
}

#[test]
fn unknown_provider_is_rejected() {
    let err =
        resolve_model_selection_for_auths("missing/gpt-5.5", "openai", "codex", &["codex".into()])
            .unwrap_err();

    assert_eq!(
        err,
        ModelSelectionError::UnknownProvider {
            provider: "missing".into()
        }
    );
}
