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

fn with_empty_provider_models_cache<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let cache_dir = unique_cache_dir(name);
    let result = with_provider_models_cache_dir_for_tests(cache_dir.clone(), f);
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
fn resolves_legacy_openrouter_model_references_to_canonical_provider() {
    with_cached_provider_models(
        "openrouter",
        vec![provider_model("openrouter", "anthropic/claude-sonnet-4")],
        || {
            let selection = resolve_model_selection_for_auths(
                "openrouter-oauth/anthropic/claude-sonnet-4",
                "openai",
                "api-key",
                &["openrouter-oauth".into()],
            )
            .unwrap();

            assert_eq!(
                selection,
                ModelSelection {
                    provider: "openrouter".into(),
                    model: "anthropic/claude-sonnet-4".into(),
                    auth: "openrouter-oauth".into(),
                    from_catalog: true,
                }
            );
        },
    );
}

#[test]
fn resolves_poolside_references_to_internal_model_id() {
    with_cached_provider_models(
        "poolside",
        vec![provider_model("poolside", "laguna-m.1")],
        || {
            let clean = resolve_model_selection_for_provider(
                "poolside",
                "laguna-m.1",
                SelectionAuthContext::none(),
            )
            .unwrap();
            let legacy = resolve_model_selection_for_provider(
                "poolside",
                "poolside/laguna-m.1",
                SelectionAuthContext::none(),
            )
            .unwrap();
            let double = resolve_model_selection_for_provider(
                "poolside",
                "poolside/poolside/laguna-m.1",
                SelectionAuthContext::none(),
            )
            .unwrap();

            assert_eq!(clean.model, "laguna-m.1");
            assert_eq!(legacy, clean);
            assert_eq!(double, clean);
        },
    );
}

#[test]
fn provider_selection_prefers_credential_backed_auth_over_default() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            let selection = resolve_model_selection_for_provider(
                "ollama-cloud",
                "glm-5.2",
                SelectionAuthContext {
                    current: None,
                    available: &["ollama-cloud-device".into()],
                },
            )
            .unwrap();

            assert_eq!(
                selection,
                ModelSelection {
                    provider: "ollama-cloud".into(),
                    model: "glm-5.2".into(),
                    auth: "ollama-cloud-device".into(),
                    from_catalog: true,
                }
            );
        },
    );
}

#[test]
fn provider_selection_keeps_current_auth_when_multiple_credentials_exist() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            // Both auth modes have credentials; the current device auth must
            // win over the earlier-registered api-key mode.
            let selection = resolve_model_selection_for_provider(
                "ollama-cloud",
                "glm-5.2",
                SelectionAuthContext {
                    current: Some("ollama-cloud-device"),
                    available: &["ollama-cloud-api-key".into(), "ollama-cloud-device".into()],
                },
            )
            .unwrap();

            assert_eq!(selection.auth, "ollama-cloud-device");
        },
    );
}

// Covers: switching to a keyless-capable host must use a stored key
// Owner: model catalog
#[test]
fn provider_selection_prefers_stored_key_over_keyless_default() {
    with_cached_provider_models("ollama", vec![provider_model("ollama", "llama3.2")], || {
        let selection = resolve_model_selection_for_provider(
            "ollama",
            "llama3.2",
            SelectionAuthContext {
                current: Some("api-key"),
                available: &["none".into(), "ollama-api-key".into()],
            },
        )
        .unwrap();

        assert_eq!(selection.auth, "ollama-api-key");
    });
}

#[test]
fn provider_selection_ignores_current_auth_from_another_provider() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            let selection = resolve_model_selection_for_provider(
                "ollama-cloud",
                "glm-5.2",
                SelectionAuthContext {
                    current: Some("kimi-api-key"),
                    available: &["ollama-cloud-device".into()],
                },
            )
            .unwrap();

            assert_eq!(selection.auth, "ollama-cloud-device");
        },
    );
}

#[test]
fn provider_selection_uses_default_auth_when_no_credentials_available() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            let selection = resolve_model_selection_for_provider(
                "ollama-cloud",
                "glm-5.2",
                SelectionAuthContext::none(),
            )
            .unwrap();

            assert_eq!(selection.auth, "ollama-cloud-api-key");
        },
    );
}

#[test]
fn bare_model_selection_prefers_credential_backed_auth_mode() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            let selection = resolve_model_selection_for_auths(
                "glm-5.2",
                "openai",
                "api-key",
                &["ollama-cloud-device".into()],
            )
            .unwrap();

            assert_eq!(selection.provider, "ollama-cloud");
            assert_eq!(selection.auth, "ollama-cloud-device");
        },
    );
}

#[test]
fn current_auth_wins_when_it_is_available_for_provider_selection() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            let available = vec!["ollama-cloud-device".into(), "ollama-cloud-api-key".into()];
            // Qualified reference carries the current auth as preference.
            let selection = resolve_model_selection_for_auths(
                "ollama-cloud/glm-5.2",
                "ollama-cloud",
                "ollama-cloud-api-key",
                &available,
            )
            .unwrap();

            assert_eq!(selection.auth, "ollama-cloud-api-key");
        },
    );
}

#[test]
fn github_copilot_requires_cached_models() {
    with_empty_provider_models_cache("github-copilot-empty", || {
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

// Covers: descriptor default wins over lexicographic first cached model when present
// Owner: model catalog
#[test]
fn preferred_cached_default_prefers_descriptor_default_when_present() {
    with_cached_provider_models(
        "anthropic",
        vec![
            provider_model("anthropic", "claude-haiku-4-5"),
            provider_model("anthropic", "claude-sonnet-4-5"),
        ],
        || {
            assert_eq!(
                default_model_for_provider("anthropic").as_deref(),
                Some("claude-sonnet-4-5")
            );
        },
    );

    with_cached_provider_models(
        "anthropic",
        vec![provider_model("anthropic", "claude-haiku-4-5")],
        || {
            assert_eq!(
                default_model_for_provider("anthropic").as_deref(),
                Some("claude-haiku-4-5")
            );
        },
    );

    with_empty_provider_models_cache("anthropic-default-empty", || {
        assert_eq!(
            default_model_for_provider("anthropic").as_deref(),
            Some("claude-sonnet-4-5")
        );
    });
}

// Covers: Meta default is muse-spark-1.2 with empty cache and prefers it when cached
// Owner: model catalog
#[test]
fn meta_default_is_muse_spark_1_2() {
    with_empty_provider_models_cache("meta-default-empty", || {
        assert_eq!(
            default_model_for_provider("meta").as_deref(),
            Some("muse-spark-1.2")
        );
        let selection = resolve_model_selection_for_provider(
            "meta",
            "muse-spark-1.2",
            SelectionAuthContext::none(),
        )
        .unwrap();
        assert_eq!(selection.model, "muse-spark-1.2");
    });

    with_cached_provider_models(
        "meta",
        vec![
            provider_model("meta", "muse-spark-1.1"),
            provider_model("meta", "muse-spark-1.2"),
        ],
        || {
            assert_eq!(
                default_model_for_provider("meta").as_deref(),
                Some("muse-spark-1.2")
            );
            let available = available_models_for_auths(&["meta-api-key".into()]);
            let meta_models = available
                .iter()
                .filter(|entry| entry.provider == "meta")
                .map(|entry| entry.model.as_str())
                .collect::<Vec<_>>();
            assert_eq!(meta_models, vec!["muse-spark-1.1", "muse-spark-1.2"]);
        },
    );
}

// Covers: OpenCode Go has no baked default when the live /models cache is empty
// Owner: model catalog
#[test]
fn opencode_go_has_no_default_model_when_cache_is_empty() {
    with_empty_provider_models_cache("opencode-go-default-empty", || {
        assert_eq!(default_model_for_provider("opencode-go"), None);
    });
}

// Covers: login groups derive single-provider rows and keep cross-provider merges
// Owner: model catalog
#[test]
fn login_groups_include_meta_and_merge_openai_codex() {
    let groups = login_groups();
    let prompts = groups
        .iter()
        .map(|group| group.prompt.as_str())
        .collect::<Vec<_>>();
    assert!(
        prompts.windows(2).all(|pair| pair[0] <= pair[1]),
        "login groups must sort by display prompt: {prompts:?}"
    );

    let meta = groups
        .iter()
        .find(|group| group.id == "meta")
        .expect("meta login group");
    assert_eq!(meta.prompt, "Meta Model API");
    assert_eq!(meta.methods.len(), 1);
    assert_eq!(meta.methods[0].target.auth, "meta-api-key");

    let opencode_go = groups
        .iter()
        .find(|group| group.id == "opencode-go")
        .expect("opencode-go login group");
    assert_eq!(opencode_go.prompt, "OpenCode Go");
    assert_eq!(opencode_go.methods.len(), 1);
    assert_eq!(opencode_go.methods[0].target.auth, "opencode-go-api-key");

    let ollama = groups
        .iter()
        .find(|group| group.id == "ollama")
        .expect("ollama login group");
    assert_eq!(ollama.prompt, "Ollama");
    assert_eq!(ollama.methods.len(), 1);
    assert_eq!(ollama.methods[0].target.auth, "ollama-api-key");

    let openai = groups
        .iter()
        .find(|group| group.id == "openai")
        .expect("openai login group");
    let openai_auths = openai
        .methods
        .iter()
        .map(|method| method.target.auth.as_str())
        .collect::<Vec<_>>();
    assert_eq!(openai_auths, vec!["api-key", "codex"]);
    assert!(groups.iter().all(|group| group.id != "openai-codex"));
    assert!(groups.iter().all(|group| group.id != "kimi-code"));
}
