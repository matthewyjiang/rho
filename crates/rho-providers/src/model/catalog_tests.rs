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

// Covers: offline enumeration includes cached credentialed and keyless models,
// while credential-scoped enumeration still excludes unavailable auth modes.
// Owner: provider catalog, not the model-prompt picker that consumes it.
#[test]
fn local_catalog_includes_cached_models_without_credentials() {
    with_empty_provider_models_cache("local-catalog", || {
        for provider in ["openai", "ollama"] {
            replace_cached_provider_models_for_tests(
                provider,
                &[provider_model(provider, "local-model")],
            )
            .unwrap();
        }
        let cached = vec![
            ModelCatalogEntry {
                provider: "ollama".into(),
                model: "local-model".into(),
                display_name: "local-model".into(),
                auth_modes: vec!["none".into(), "ollama-api-key".into()],
            },
            ModelCatalogEntry {
                provider: "openai".into(),
                model: "local-model".into(),
                display_name: "local-model".into(),
                auth_modes: vec!["api-key".into()],
            },
        ];
        let mut expected = cached.clone();
        expected.extend_from_slice(model_catalog());
        expected.sort_by(|left, right| {
            (&left.provider, &left.model).cmp(&(&right.provider, &right.model))
        });
        assert_eq!(locally_known_models(), expected);
        assert_eq!(available_models_for_auths(&[]), vec![]);
        let mut keyless = cached[0].clone();
        keyless.auth_modes = vec!["none".into()];
        assert_eq!(available_models_for_auths(&["none".into()]), vec![keyless]);
        assert_eq!(
            available_models_for_auths(&["api-key".into()]),
            vec![cached[1].clone()]
        );
    });
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

// Covers: ollama-cloud auth preference: a credential-backed mode beats the
// default, the current auth wins only when it has credentials for this
// provider, and the descriptor default is the last resort
// Owner: model catalog
#[test]
fn provider_selection_prefers_current_then_credential_backed_auth() {
    with_cached_provider_models(
        "ollama-cloud",
        vec![provider_model("ollama-cloud", "glm-5.2")],
        || {
            let device = "ollama-cloud-device";
            let api_key = "ollama-cloud-api-key";
            for (case, current, available, expected) in [
                ("credential over default", None, vec![device], device),
                (
                    "current among several",
                    Some(device),
                    vec![api_key, device],
                    device,
                ),
                (
                    "current from another provider",
                    Some("kimi-api-key"),
                    vec![device],
                    device,
                ),
                ("no credentials", None, vec![], api_key),
            ] {
                let available = available.into_iter().map(String::from).collect::<Vec<_>>();
                let selection = resolve_model_selection_for_provider(
                    "ollama-cloud",
                    "glm-5.2",
                    SelectionAuthContext {
                        current,
                        available: &available,
                    },
                )
                .unwrap();
                assert_eq!(selection.auth, expected, "{case}");
            }

            let bare =
                resolve_model_selection_for_auths("glm-5.2", "openai", "api-key", &[device.into()])
                    .unwrap();
            assert_eq!(
                (bare.provider.as_str(), bare.auth.as_str()),
                ("ollama-cloud", device)
            );

            let qualified = resolve_model_selection_for_auths(
                "ollama-cloud/glm-5.2",
                "ollama-cloud",
                api_key,
                &[device.into(), api_key.into()],
            )
            .unwrap();
            assert_eq!(qualified.auth, api_key);
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
                current: Some("none"),
                available: &["none".into(), "ollama-api-key".into()],
            },
        )
        .unwrap();

        assert_eq!(selection.auth, "ollama-api-key");
    });
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

// Covers: each selection rejection names its rule: uncached model on the
// current provider, codex allowlist, unknown provider, and decision-only hosts
// that are not chat models
// Owner: model catalog
#[test]
fn selection_rejections_report_the_failing_rule() {
    let refresh_hint = "Open /config and choose Refresh model lists to update available models.";
    for (input, current_provider, auth, available, expected) in [
        (
            "brand-new-model",
            "openai",
            "codex",
            "codex",
            ModelSelectionError::UnavailableModel {
                provider: "openai".into(),
                model: "brand-new-model".into(),
                hint: refresh_hint,
            },
        ),
        (
            "openai-codex/custom-model",
            "openai-codex",
            "codex",
            "codex",
            ModelSelectionError::UnavailableModel {
                provider: "openai-codex".into(),
                model: "custom-model".into(),
                hint: "Choose a model from the provider allowlist.",
            },
        ),
        (
            "missing/gpt-5.5",
            "openai",
            "codex",
            "codex",
            ModelSelectionError::UnknownProvider {
                provider: "missing".into(),
            },
        ),
        (
            "typesafe/jev-latest",
            "openai",
            "codex",
            "typesafe-api-key",
            ModelSelectionError::DecisionOnly {
                provider: "typesafe".into(),
            },
        ),
    ] {
        assert_eq!(
            resolve_model_selection_for_auths(input, current_provider, auth, &[available.into()])
                .unwrap_err(),
            expected,
            "{input}"
        );
    }
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
        // OpenCode Go has no baked default to fall back to.
        assert_eq!(default_model_for_provider("opencode-go"), None);
    });
}

// Covers: baked provider defaults resolve with an empty cache and stay the
// default when the live /models cache also lists older models
// Owner: model catalog
#[test]
fn baked_provider_defaults_survive_empty_and_cached_models() {
    for (provider, default, older, auth) in [
        ("meta", "muse-spark-1.2", "muse-spark-1.1", "meta-api-key"),
        ("minimax", "MiniMax-M3", "MiniMax-M2.7", "minimax-api-key"),
    ] {
        with_empty_provider_models_cache(&format!("{provider}-default-empty"), || {
            assert_eq!(
                default_model_for_provider(provider).as_deref(),
                Some(default),
                "{provider}"
            );
            let selection = resolve_model_selection_for_provider(
                provider,
                default,
                SelectionAuthContext::none(),
            )
            .unwrap();
            assert_eq!(
                (selection.model.as_str(), selection.auth.as_str()),
                (default, auth),
                "{provider}"
            );
        });

        with_cached_provider_models(
            provider,
            vec![
                provider_model(provider, older),
                provider_model(provider, default),
            ],
            || {
                assert_eq!(
                    default_model_for_provider(provider).as_deref(),
                    Some(default),
                    "{provider}"
                );
                let available = available_models_for_auths(&[auth.into()]);
                let provider_models = available
                    .iter()
                    .filter(|entry| entry.provider == provider)
                    .map(|entry| entry.model.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(provider_models, vec![older, default], "{provider}");
            },
        );
    }
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
    assert_eq!(meta.methods.len(), 2);
    assert_eq!(meta.methods[0].target.auth, "meta-api-key");
    assert_eq!(meta.methods[0].prompt, "API Key");
    assert_eq!(meta.methods[1].target.auth, "meta-muse");
    assert_eq!(meta.methods[1].prompt, "Subscription");

    // Both OpenCode gateways take an API key, so the group names each method.
    let opencode = groups
        .iter()
        .find(|group| group.id == "opencode")
        .expect("opencode login group");
    assert_eq!(opencode.prompt, "OpenCode");
    assert_eq!(
        opencode
            .methods
            .iter()
            .map(|method| (method.prompt.as_str(), method.target.auth.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Zen pay-as-you-go", "opencode-api-key"),
            ("Go subscription", "opencode-go-api-key"),
        ]
    );
    assert!(groups.iter().all(|group| group.id != "opencode-go"));

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
