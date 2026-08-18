use super::*;
use crate::model::{
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ReasoningLevelSet,
};
use pretty_assertions::assert_eq;
use serde_json::{json, Value};

fn catalog(api: &Value) -> super::document::ModelsDevCatalog {
    super::document::ModelsDevCatalog::from_json_value(api)
}

fn upstream_metadata_from_api(api: &Value, provider: &str, model: &str) -> Option<ModelMetadata> {
    super::upstream_metadata_from_api(&catalog(api), provider, model)
}

fn deprecated_provider_models_from_api(api: &Value, provider: &str) -> HashSet<String> {
    super::document::deprecated_provider_models(&catalog(api), provider)
}

fn hydrate_catalog_from_api(api: &Value) -> usize {
    hydrate::hydrate_catalog_from_api(&catalog(api))
}

// Covers: one junk models.dev row or field type must not fail the catalog
// Owner: models.dev catalog parse
#[test]
fn malformed_catalog_rows_stay_lenient() {
    let api = json!({
        "openai": {
            "npm": "@ai-sdk/openai",
            "models": {
                "good": { "name": "Good", "reasoning": false },
                "not-object": "nope",
                "bad-fields": {
                    "name": 7,
                    "reasoning": "yes",
                    "limit": { "context": "big", "output": 128 },
                    "cost": "free",
                    "reasoning_options": { "type": "effort" }
                },
                "bad-option": {
                    "name": "Bad option",
                    "reasoning": true,
                    "reasoning_options": ["toggle"]
                }
            }
        },
        "not-a-provider": ["skip"]
    });

    assert_eq!(
        model_metadata_from_api(&api, "openai", "good").and_then(|metadata| metadata.display_name),
        Some("Good".to_string())
    );
    assert_eq!(model_metadata_from_api(&api, "openai", "not-object"), None);

    let bad = model_metadata_from_api(&api, "openai", "bad-fields").unwrap();
    assert_eq!(
        bad,
        ModelMetadata {
            display_name: None,
            advertised_context_window: None,
            effective_context_window: None,
            usable_context_window: None,
            long_context_threshold: None,
            max_output_tokens: Some(128),
            cost_default: None,
            cost_long_context: None,
            supported_reasoning_levels: None,
            reasoning_off_behavior: ReasoningOffBehavior::Omit,
            reasoning_capabilities_known: false,
            reasoning_metadata_complete: false,
            sdk_package: Some("@ai-sdk/openai".to_string()),
        }
    );

    let bad_option = model_metadata_from_api(&api, "openai", "bad-option").unwrap();
    assert_eq!(bad_option.display_name, Some("Bad option".to_string()));
    assert_eq!(bad_option.supported_reasoning_levels, None);
    assert!(!bad_option.reasoning_capabilities_known);
    assert!(!bad_option.reasoning_metadata_complete);
}

// Covers: first effort option wins; a missing values list still uses toggle
// Owner: models.dev catalog parse
#[test]
fn first_effort_option_wins_and_missing_values_use_toggle() {
    let first_effort = json!({
        "openai": {
            "models": {
                "gpt-test": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "effort", "values": ["low"]},
                        {"type": "effort", "values": ["high"]},
                    ]
                }
            }
        }
    });
    assert_eq!(
        model_metadata_from_api(&first_effort, "openai", "gpt-test")
            .and_then(|metadata| metadata.supported_reasoning_levels),
        Some(vec![ReasoningLevel::Low])
    );

    let effort_without_values = json!({
        "openai": {
            "models": {
                "gpt-test": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "effort"},
                        {"type": "toggle"},
                    ]
                }
            }
        }
    });
    let metadata = model_metadata_from_api(&effort_without_values, "openai", "gpt-test").unwrap();
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Off, ReasoningLevel::Max])
    );
    assert!(metadata.reasoning_metadata_complete);
}

#[test]
fn deprecated_provider_models_only_returns_exact_deprecation_flags() {
    let api = json!({
        "google": {
            "models": {
                "gemini-active": {},
                "gemini-alpha": {"status": "alpha"},
                "gemini-beta": {"status": "beta"},
                "gemini-retired": {"status": "deprecated"}
            }
        }
    });

    assert_eq!(
        deprecated_provider_models_from_api(&api, "google"),
        HashSet::from(["gemini-retired".to_string()])
    );
    assert_eq!(
        deprecated_provider_models_from_api(&api, "missing"),
        HashSet::new()
    );
}

#[test]
fn models_dev_parses_the_catalog_name_and_rejects_blank_ones() {
    for (name, expected) in [
        (json!("GPT-5.6 Sol"), Some("GPT-5.6 Sol".to_string())),
        (json!("  GPT-5.6 Sol  "), Some("GPT-5.6 Sol".to_string())),
        (json!("   "), None),
        (json!(null), None),
        (json!(7), None),
    ] {
        let api = json!({
            "openai": { "models": { "gpt-5.6-sol": { "name": name } } }
        });

        let metadata = model_metadata_from_api(&api, "openai", "gpt-5.6-sol").unwrap();

        assert_eq!(metadata.display_name, expected);
    }

    let nameless = json!({ "openai": { "models": { "gpt-5.6-sol": {} } } });
    assert_eq!(
        model_metadata_from_api(&nameless, "openai", "gpt-5.6-sol")
            .unwrap()
            .display_name,
        None
    );
}

// Covers: models.dev npm inherits from the provider document unless a model overrides it
// Owner: models.dev catalog policy
#[test]
fn models_dev_resolves_sdk_package_from_provider_and_model_npm() {
    use super::CatalogSdkAdapter;

    let api = json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {
                "kimi-k2.7-code": {
                    "name": "Kimi K2.7 Code",
                    "reasoning": true,
                    "reasoning_options": []
                },
                "grok-4.5": {
                    "name": "Grok 4.5",
                    "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["low", "high"]}],
                    "provider": { "npm": "@ai-sdk/openai" }
                },
                "minimax-m3": {
                    "name": "MiniMax-M3",
                    "reasoning": true,
                    "reasoning_options": [{"type": "toggle"}],
                    "npm": "@ai-sdk/anthropic"
                }
            }
        }
    });

    let inherited = model_metadata_from_api(&api, "opencode-go", "kimi-k2.7-code").unwrap();
    let responses = model_metadata_from_api(&api, "opencode-go", "grok-4.5").unwrap();
    let messages = model_metadata_from_api(&api, "opencode-go", "minimax-m3").unwrap();

    assert_eq!(
        inherited.sdk_package.as_deref(),
        Some("@ai-sdk/openai-compatible")
    );
    assert_eq!(responses.sdk_package.as_deref(), Some("@ai-sdk/openai"));
    assert_eq!(messages.sdk_package.as_deref(), Some("@ai-sdk/anthropic"));
    assert_eq!(
        inherited.cost_default, None,
        "cost still comes from catalog fields, not npm"
    );
    assert_eq!(
        [
            CatalogSdkAdapter::from_sdk_package(inherited.sdk_package.as_deref()),
            CatalogSdkAdapter::from_sdk_package(responses.sdk_package.as_deref()),
            CatalogSdkAdapter::from_sdk_package(messages.sdk_package.as_deref()),
            CatalogSdkAdapter::from_sdk_package(None),
            CatalogSdkAdapter::from_sdk_package(Some("@ai-sdk/unknown")),
        ],
        [
            CatalogSdkAdapter::OpenAiCompatible,
            CatalogSdkAdapter::OpenAiResponses,
            CatalogSdkAdapter::AnthropicMessages,
            CatalogSdkAdapter::OpenAiCompatible,
            CatalogSdkAdapter::OpenAiCompatible,
        ]
    );
}

#[test]
fn provider_facing_cache_keys_are_order_independent() {
    let api = json!({
        "anthropic": {
            "models": {
                "claude-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "high"]
                    }]
                }
            }
        }
    });
    let anthropic = upstream_metadata_from_api(&api, "anthropic", "claude-test").unwrap();
    let openrouter =
        upstream_metadata_from_api(&api, "openrouter", "anthropic/claude-test").unwrap();
    assert_eq!(
        anthropic.reasoning_capabilities(),
        ReasoningCapabilities::Unknown
    );
    assert_eq!(
        openrouter.reasoning_capabilities(),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High,
        ]))
    );

    for (name, writes) in [
        ("anthropic-first", [(&anthropic, &openrouter)]),
        ("openrouter-first", [(&openrouter, &anthropic)]),
    ] {
        let cache = tempfile::tempdir().unwrap();
        with_models_dev_cache_dir(cache.path().to_path_buf(), || {
            let (first, second) = writes[0];
            if name == "anthropic-first" {
                write_cached_upstream_model_metadata("anthropic", "claude-test", first);
                write_cached_upstream_model_metadata("openrouter", "anthropic/claude-test", second);
            } else {
                write_cached_upstream_model_metadata("openrouter", "anthropic/claude-test", first);
                write_cached_upstream_model_metadata("anthropic", "claude-test", second);
            }

            assert_eq!(
                cached_upstream_model_metadata("anthropic", "claude-test")
                    .unwrap()
                    .reasoning_capabilities(),
                ReasoningCapabilities::Unknown
            );
            assert_eq!(
                cached_upstream_model_metadata("openrouter", "anthropic/claude-test")
                    .unwrap()
                    .reasoning_capabilities(),
                openrouter.reasoning_capabilities()
            );
        });
    }
}

#[test]
fn stale_rows_remain_available_as_offline_fallback() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        let stale = ModelMetadata {
            advertised_context_window: Some(200_000),
            reasoning_capabilities_known: false,
            reasoning_metadata_complete: false,
            ..ModelMetadata::default()
        };
        write_cached_upstream_model_metadata("anthropic", "claude-test", &stale);
        assert_eq!(
            current_cached_upstream_model_metadata("anthropic", "claude-test"),
            None
        );
        assert_eq!(
            cached_upstream_model_metadata("anthropic", "claude-test"),
            Some(stale)
        );

        let stale_exact = ModelMetadata {
            supported_reasoning_levels: Some(vec![ReasoningLevel::Low, ReasoningLevel::High]),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: false,
            ..ModelMetadata::default()
        };
        write_cached_upstream_model_metadata("xai", "stale-exact", &stale_exact);
        assert_eq!(
            cached_reasoning_capabilities("xai", "stale-exact"),
            stale_exact.reasoning_capabilities()
        );
        assert_eq!(
            current_reasoning_capabilities("xai", "stale-exact"),
            ReasoningCapabilities::Unknown
        );
    });
}

#[test]
fn poolside_version_five_metadata_is_stale_after_reasoning_policy_change() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        let old_metadata = ModelMetadata {
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };
        write_cached_upstream_model_metadata("poolside", "laguna-s-2.1", &old_metadata);
        open_models_dev_cache()
            .unwrap()
            .execute(
                "update model_metadata set cache_version = 5
                 where provider = 'poolside' and model = 'laguna-s-2.1'",
                [],
            )
            .unwrap();

        assert_eq!(
            current_cached_upstream_model_metadata("poolside", "laguna-s-2.1"),
            None
        );
        assert_eq!(
            cached_upstream_model_metadata("poolside", "laguna-s-2.1"),
            Some(old_metadata)
        );
    });
}

// Covers: Unknown-era Token Plan rows must not block ExactAdvertised rehydrate
// Owner: models.dev cache freshness
#[test]
fn qwen_token_plan_unknown_complete_rows_are_stale_after_policy_change() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        // Shape written while catalog_reasoning was Unknown: complete, but no levels.
        let old_metadata = ModelMetadata {
            reasoning_capabilities_known: false,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };
        write_cached_upstream_model_metadata("qwen-token-plan", "qwen3.8-max", &old_metadata);
        open_models_dev_cache()
            .unwrap()
            .execute(
                "update model_metadata set cache_version = 6
                 where provider = 'qwen-token-plan' and model = 'qwen3.8-max'",
                [],
            )
            .unwrap();

        assert_eq!(
            current_cached_upstream_model_metadata("qwen-token-plan", "qwen3.8-max"),
            None
        );
        assert!(model_metadata_needs_refresh(
            "qwen-token-plan",
            "qwen3.8-max"
        ));
        assert_eq!(
            cached_upstream_model_metadata("qwen-token-plan", "qwen3.8-max"),
            Some(old_metadata)
        );
    });
}

#[test]
fn provider_context_length_overrides_generic_effective_context() {
    let cache_dir = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache_dir.path().to_path_buf(), || {
        let fallback = apply_overrides(
            "kimi-code",
            "k3",
            ModelMetadata {
                advertised_context_window: Some(1_048_576),
                effective_context_window: Some(1_048_576),
                ..ModelMetadata::default()
            },
        );
        assert_eq!(fallback.effective_context_window, Some(262_144));

        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "k3".into(),
                display_name: "Kimi K3".into(),
                context_window: Some(262_144),
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();

        let metadata = apply_overrides(
            "kimi-code",
            "k3",
            ModelMetadata {
                advertised_context_window: Some(1_048_576),
                effective_context_window: Some(1_048_576),
                ..ModelMetadata::default()
            },
        );

        assert_eq!(metadata.advertised_context_window, Some(1_048_576));
        assert_eq!(metadata.effective_context_window, Some(262_144));
        assert_eq!(metadata.display_context_window(), Some(262_144));
    });
}

#[test]
fn exact_catalog_toggle_does_not_imply_off() {
    let api = json!({
        "moonshotai": {
            "models": {
                "kimi-k3": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "toggle"},
                        {"type": "effort", "values": ["low", "high", "max"]}
                    ]
                }
            }
        },
        "xai": {
            "models": {
                "grok-4.5": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "effort", "values": ["low", "medium", "high"]}
                    ]
                },
                "grok-4.6": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "effort", "values": ["low", "medium", "high"]}
                    ]
                },
                "grok-4.3": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "toggle"},
                        {"type": "effort", "values": ["low", "high"]}
                    ]
                }
            }
        }
    });

    assert_eq!(
        model_metadata_from_api(&api, "moonshotai", "kimi-k3")
            .unwrap()
            .supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Low,
            ReasoningLevel::High,
            ReasoningLevel::Max,
        ])
    );
    assert_eq!(
        model_metadata_from_api(&api, "xai", "grok-4.5")
            .unwrap()
            .supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ])
    );
    assert_eq!(
        model_metadata_from_api(&api, "xai", "grok-4.6")
            .unwrap()
            .supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ])
    );
    assert_eq!(
        model_metadata_from_api(&api, "xai", "grok-4.3")
            .unwrap()
            .supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High,
        ])
    );
}

#[test]
fn provider_path_that_ignores_reasoning_is_not_configurable() {
    let api = json!({
        "github-copilot": {
            "models": {
                "gpt-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["none", "low", "high"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "github-copilot", "gpt-test").unwrap();
    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(
        metadata.reasoning_capabilities(),
        ReasoningCapabilities::NotConfigurable
    );
}

#[test]
fn parses_reasoning_effort_options() {
    let api = serde_json::json!({
        "openai": {
            "models": {
                "gpt-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["none", "low", "high", "xhigh"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert_eq!(
        metadata.reasoning_off_behavior,
        ReasoningOffBehavior::EffortNone
    );
    assert_eq!(metadata.reasoning_effort(ReasoningLevel::Off), Some("none"));
    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ])
    );
}

#[test]
fn effort_options_without_none_do_not_inject_off_for_openai() {
    let api = serde_json::json!({
        "openai": {
            "models": {
                "gpt-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "medium", "high", "xhigh"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert_eq!(metadata.reasoning_off_behavior, ReasoningOffBehavior::Omit);
    assert_eq!(metadata.reasoning_effort(ReasoningLevel::Off), None);
    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ])
    );
}

#[test]
fn mixed_known_and_unknown_efforts_leave_capabilities_incomplete() {
    let api = json!({
        "xai": {
            "models": {
                "grok-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "turbo", "high"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "xai", "grok-test").unwrap();

    assert!(!metadata.reasoning_capabilities_known);
    assert!(!metadata.reasoning_metadata_complete);
    assert_eq!(
        metadata.reasoning_capabilities(),
        ReasoningCapabilities::Unknown
    );
}

#[test]
fn models_without_effort_choices_are_not_configurable() {
    let api = serde_json::json!({
        "openai": {
            "models": {
                "gpt-test": {"reasoning": true, "reasoning_options": []}
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
    assert_eq!(
        ReasoningCapabilities::from_metadata(
            metadata.supported_reasoning_levels,
            metadata.reasoning_capabilities_known,
        ),
        ReasoningCapabilities::NotConfigurable
    );
}

#[test]
fn anthropic_effort_catalog_stays_unknown_until_protocols_are_modeled() {
    let api = json!({
        "anthropic": {
            "models": {
                "claude-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "medium", "high"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "anthropic", "claude-test").unwrap();
    assert_eq!(
        metadata.reasoning_capabilities(),
        ReasoningCapabilities::Unknown
    );
    assert!(metadata.reasoning_metadata_complete);
    assert!(!should_rehydrate_cached_metadata(
        MODEL_METADATA_CACHE_VERSION,
        &metadata
    ));
}

#[test]
fn leaves_unknown_reasoning_option_schemas_unknown() {
    let api = serde_json::json!({
        "anthropic": {
            "models": {
                "claude-test": {
                    "reasoning": true,
                    "reasoning_options": [{"type": "budget_tokens", "min": 1024}]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "anthropic", "claude-test").unwrap();

    assert!(!metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
}

#[test]
fn non_reasoning_models_are_not_configurable() {
    let api = serde_json::json!({
        "openai": {"models": {"gpt-test": {"reasoning": false}}}
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
    assert_eq!(
        ReasoningCapabilities::from_metadata(
            metadata.supported_reasoning_levels,
            metadata.reasoning_capabilities_known,
        ),
        ReasoningCapabilities::NotConfigurable
    );
}

#[test]
fn reasoning_models_without_options_are_not_capability_complete() {
    let api = json!({
        "xai": {
            "models": {
                "grok-4.5": {
                    "reasoning": true,
                    "limit": { "context": 500000, "output": 500000 }
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "xai", "grok-4.5").unwrap();

    assert!(!metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
}

#[test]
fn models_dev_parses_long_context_cost_tiers() {
    let api = json!({
        "xai": {
            "models": {
                "grok-4.5": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "medium", "high"]
                    }],
                    "limit": { "context": 500000, "output": 500000 },
                    "cost": {
                        "input": 2.0,
                        "output": 6.0,
                        "cache_read": 0.5,
                        "tiers": [{
                            "input": 4.0,
                            "output": 12.0,
                            "cache_read": 1.0,
                            "tier": { "type": "context", "size": 200000 }
                        }],
                        // Different rates so a context_over-first parse cannot pass.
                        "context_over_200k": {
                            "input": 9.0,
                            "output": 18.0,
                            "cache_read": 3.0
                        }
                    }
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "xai", "grok-4.5").unwrap();

    assert_eq!(
        metadata,
        ModelMetadata {
            display_name: None,
            advertised_context_window: Some(500_000),
            effective_context_window: Some(500_000),
            usable_context_window: None,
            long_context_threshold: Some(200_000),
            max_output_tokens: Some(500_000),
            cost_default: Some(ModelCost {
                input_micros_per_m: Some(2_000_000),
                output_micros_per_m: Some(6_000_000),
                cache_read_micros_per_m: Some(500_000),
                cache_write_micros_per_m: None,
            }),
            cost_long_context: Some(ModelCost {
                input_micros_per_m: Some(4_000_000),
                output_micros_per_m: Some(12_000_000),
                cache_read_micros_per_m: Some(1_000_000),
                cache_write_micros_per_m: None,
            }),
            supported_reasoning_levels: Some(vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
            ]),
            reasoning_off_behavior: ReasoningOffBehavior::Omit,
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            sdk_package: None,
        }
    );
    assert_eq!(
        metadata
            .cost_for_input_tokens(200_001)
            .unwrap()
            .input_micros_per_m,
        Some(4_000_000)
    );
    assert_eq!(
        metadata
            .cost_for_input_tokens(200_000)
            .unwrap()
            .input_micros_per_m,
        Some(2_000_000)
    );
}

#[test]
fn rehydrates_when_cache_version_is_stale_or_metadata_is_incomplete() {
    let complete = ModelMetadata {
        supported_reasoning_levels: Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]),
        reasoning_capabilities_known: true,
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    };
    let missing_flag = ModelMetadata {
        supported_reasoning_levels: Some(vec![ReasoningLevel::Off, ReasoningLevel::High]),
        reasoning_capabilities_known: false,
        ..ModelMetadata::default()
    };
    let intentional_unknown = ModelMetadata {
        supported_reasoning_levels: None,
        reasoning_capabilities_known: false,
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    };
    let sealed_null_without_flag = ModelMetadata {
        supported_reasoning_levels: None,
        reasoning_capabilities_known: false,
        ..ModelMetadata::default()
    };

    assert!(should_rehydrate_cached_metadata(1, &complete));
    assert!(should_rehydrate_cached_metadata(
        MODEL_METADATA_CACHE_VERSION,
        &missing_flag
    ));
    assert!(should_rehydrate_cached_metadata(
        MODEL_METADATA_CACHE_VERSION,
        &sealed_null_without_flag
    ));
    assert!(!should_rehydrate_cached_metadata(
        MODEL_METADATA_CACHE_VERSION,
        &complete
    ));
    // Provider policies may intentionally resolve complete metadata to Unknown;
    // those rows must not thrash on rehydrate.
    assert!(!should_rehydrate_cached_metadata(
        MODEL_METADATA_CACHE_VERSION,
        &intentional_unknown
    ));
}

#[test]
fn authenticated_provider_levels_replace_generic_catalog_levels() {
    let cache_dir = std::env::temp_dir().join(format!(
        "rho-models-dev-provider-reasoning-{}",
        std::process::id()
    ));
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "k3".into(),
                display_name: "Kimi K3".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Levels(
                    crate::model::ReasoningLevelSet::new(vec![
                        ReasoningLevel::Off,
                        ReasoningLevel::Low,
                        ReasoningLevel::High,
                        ReasoningLevel::Max,
                    ]),
                ),
            }],
        )
        .unwrap();

        let metadata = apply_overrides(
            "kimi-code",
            "k3",
            ModelMetadata {
                supported_reasoning_levels: Some(vec![ReasoningLevel::Off, ReasoningLevel::Max]),
                reasoning_capabilities_known: true,
                ..ModelMetadata::default()
            },
        );

        assert_eq!(
            metadata.supported_reasoning_levels,
            Some(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])
        );
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn local_reasoning_override_replaces_provider_levels_exactly() {
    let provider_metadata = ModelMetadata {
        supported_reasoning_levels: Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High,
            ReasoningLevel::Max,
        ]),
        reasoning_capabilities_known: true,
        ..ModelMetadata::default()
    };
    let table =
        toml::from_str::<toml::Value>(r#"supported_reasoning_levels = ["medium", "xhigh"]"#)
            .unwrap();

    let metadata = overrides::merge_toml_override(provider_metadata, table.as_table().unwrap());

    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Medium, ReasoningLevel::Xhigh])
    );
    assert!(metadata.reasoning_capabilities_known);
}

// Covers: Token Plan qwen3.8-max must expose only models.dev effort levels
// Owner: models.dev catalog policy
#[test]
fn qwen_token_plan_qwen38_max_uses_exact_advertised_efforts() {
    let api = json!({
        "alibaba-token-plan": {
            "models": {
                "qwen3.8-max": {
                    "reasoning": true,
                    "reasoning_options": [
                        { "type": "toggle" },
                        {
                            "type": "effort",
                            "values": ["low", "medium", "xhigh"]
                        },
                        {
                            "type": "budget_tokens",
                            "min": 0,
                            "max": 262144
                        }
                    ]
                },
                "qwen3.8-max-preview": {
                    "reasoning": true,
                    "reasoning_options": [
                        {
                            "type": "effort",
                            "values": ["low", "medium", "xhigh"]
                        }
                    ]
                }
            }
        }
    });

    let max = upstream_metadata_from_api(&api, "qwen-token-plan", "qwen3.8-max").unwrap();
    assert_eq!(
        max.reasoning_capabilities(),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::Xhigh,
        ]))
    );

    let preview =
        upstream_metadata_from_api(&api, "qwen-token-plan", "qwen3.8-max-preview").unwrap();
    assert_eq!(
        preview.reasoning_capabilities(),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::Xhigh,
        ]))
    );
}

// Covers: UI prefers current-known capabilities and only then a stale-known cache row
// Owner: models.dev capability lookup
#[test]
fn known_reasoning_capabilities_prefers_current_then_stale_known() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        let stale_exact = ModelMetadata {
            supported_reasoning_levels: Some(vec![ReasoningLevel::Low, ReasoningLevel::High]),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: false,
            ..ModelMetadata::default()
        };
        write_cached_upstream_model_metadata("xai", "stale-exact", &stale_exact);

        assert_eq!(
            current_reasoning_capabilities("xai", "stale-exact"),
            ReasoningCapabilities::Unknown
        );
        assert_eq!(
            known_reasoning_capabilities("xai", "stale-exact"),
            stale_exact.reasoning_capabilities()
        );
        assert_eq!(
            known_reasoning_metadata("xai", "stale-exact")
                .map(|metadata| metadata.supported_reasoning_levels),
            Some(stale_exact.supported_reasoning_levels.clone())
        );

        let current_exact = ModelMetadata {
            supported_reasoning_levels: Some(vec![
                ReasoningLevel::Minimal,
                ReasoningLevel::Medium,
                ReasoningLevel::Xhigh,
            ]),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };
        write_cached_upstream_model_metadata("xai", "current-exact", &current_exact);
        assert_eq!(
            known_reasoning_capabilities("xai", "current-exact"),
            current_exact.reasoning_capabilities()
        );

        assert_eq!(
            known_reasoning_capabilities("xai", "missing-model"),
            ReasoningCapabilities::Unknown
        );
        assert_eq!(known_reasoning_metadata("xai", "missing-model"), None);
    });
}

// Covers: a provider whose models live under a different models.dev key still
// resolves names. `openai-codex` sells OpenAI models through Codex OAuth and
// has no models.dev entry of its own; it reads `openai` upstream.
#[test]
fn providers_that_read_another_upstream_catalog_still_get_names() {
    let api = json!({
        "openai": {
            "models": {
                "gpt-5.6-luna": {
                    "name": "GPT-5.6 Luna",
                    "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["low", "high"]}]
                }
            }
        }
    });

    let metadata = upstream_metadata_from_api(&api, "openai-codex", "gpt-5.6-luna")
        .expect("openai-codex reads the openai catalog");

    assert_eq!(metadata.display_name.as_deref(), Some("GPT-5.6 Luna"));
    assert!(metadata.reasoning_metadata_complete);

    // The row is cached and read back under the Rho provider name, not the
    // upstream one, so a lookup for `openai-codex` finds it.
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        write_cached_upstream_model_metadata("openai-codex", "gpt-5.6-luna", &metadata);

        assert_eq!(
            cached_model_metadata("openai-codex", "gpt-5.6-luna")
                .and_then(|metadata| metadata.display_name)
                .as_deref(),
            Some("GPT-5.6 Luna")
        );
        // A model with no cached row has no name, even though provider
        // capability fallbacks still give it other metadata.
        assert_eq!(
            cached_model_metadata("openai-codex", "gpt-5.6-terra")
                .and_then(|metadata| metadata.display_name),
            None
        );
    });
}

// Covers: the prefetch must skip the network when a full snapshot is already
// current. Startup calls it on every launch.
// Owner: models.dev catalog prefetch
#[tokio::test]
async fn prefetch_does_nothing_when_every_target_is_current() {
    let cache = tempfile::tempdir().unwrap();
    let current = ModelMetadata {
        display_name: Some("GPT-5.6 Luna".into()),
        supported_reasoning_levels: Some(vec![ReasoningLevel::Low, ReasoningLevel::High]),
        reasoning_capabilities_known: true,
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    };

    // The cache dir is thread-local, so the write and the check share a thread.
    // A network call would be the only way this could fail offline.
    let written = with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        write_cached_upstream_model_metadata("openai-codex", "gpt-5.6-luna", &current);
        mark_catalog_snapshot_current_for_tests();
        // Duplicates collapse before any freshness check.
        let targets = vec![
            ("openai-codex".to_string(), "gpt-5.6-luna".to_string()),
            ("openai-codex".to_string(), "gpt-5.6-luna".to_string()),
        ];
        futures_util::future::FutureExt::now_or_never(prefetch_model_metadata(targets))
    });

    assert_eq!(
        written,
        Some(0),
        "a fully current target list must resolve without awaiting the network"
    );
}

// Covers: one models.dev document hydrates every registered provider that
// reads that upstream, including provider-facing aliases (openai-codex) and
// NotConfigurable providers that still have catalog names.
// Owner: models.dev full catalog hydrate
#[test]
fn hydrate_writes_complete_rows_for_every_registered_provider() {
    let api = json!({
        "openai": {
            "models": {
                "gpt-hydrate": {
                    "name": "GPT Hydrate",
                    "reasoning": false
                }
            }
        },
        "github-copilot": {
            "models": {
                "copilot-hydrate": {
                    "name": "Copilot Hydrate",
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["none", "low", "high"]
                    }]
                }
            }
        },
        "moonshotai": {
            "models": {
                "kimi-k3": {
                    "name": "Kimi K3",
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "high", "max"]
                    }]
                }
            }
        }
    });

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        let written = hydrate_catalog_from_api(&api);
        assert!(
            written >= 3,
            "expected multiple provider-facing rows, wrote {written}"
        );

        assert_eq!(
            cached_model_metadata("openai", "gpt-hydrate")
                .and_then(|metadata| metadata.display_name)
                .as_deref(),
            Some("GPT Hydrate")
        );
        assert_eq!(
            cached_model_metadata("openai-codex", "gpt-hydrate")
                .and_then(|metadata| metadata.display_name)
                .as_deref(),
            Some("GPT Hydrate")
        );
        assert_eq!(
            cached_model_metadata("github-copilot", "copilot-hydrate")
                .and_then(|metadata| metadata.display_name)
                .as_deref(),
            Some("Copilot Hydrate")
        );
        // Provider-facing Kimi alias beside the upstream catalog id.
        assert_eq!(
            cached_model_metadata("kimi-code", "k3")
                .and_then(|metadata| metadata.display_name)
                .as_deref(),
            Some("Kimi K3")
        );
    });
}

// Covers: OpenCode Go toggle-only rows still hydrate when they name an SDK package
// Owner: models.dev catalog policy
#[test]
fn hydrate_writes_opencode_go_rows_that_only_advertise_sdk_package() {
    let api = json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {
                "minimax-m3": {
                    "name": "MiniMax-M3",
                    "reasoning": true,
                    "reasoning_options": [{"type": "toggle"}],
                    "provider": { "npm": "@ai-sdk/anthropic" },
                    "limit": { "context": 1_000_000, "output": 131_072 },
                    "cost": { "input": 0.3, "output": 1.2 }
                }
            }
        }
    });

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        assert!(hydrate_catalog_from_api(&api) >= 1);
        let metadata = cached_model_metadata("opencode-go", "minimax-m3").expect("hydrated");
        assert_eq!(metadata.sdk_package.as_deref(), Some("@ai-sdk/anthropic"));
        assert_eq!(
            metadata
                .cost_default
                .and_then(|cost| cost.input_micros_per_m),
            Some(300_000)
        );
        assert!(metadata.reasoning_metadata_complete);
        assert_eq!(
            metadata.supported_reasoning_levels,
            Some(vec![ReasoningLevel::Off, ReasoningLevel::Max])
        );
    });
}

// Covers: batch metadata writes insert all records in one transaction
// Owner: models.dev cache
#[test]
fn batch_metadata_writes_inserts_all_records() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        let meta_a = ModelMetadata {
            display_name: Some("Model A".into()),
            advertised_context_window: Some(100_000),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };
        let meta_b = ModelMetadata {
            display_name: Some("Model B".into()),
            advertised_context_window: Some(200_000),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };

        let written = write_cached_upstream_model_metadata_batch([
            ("provider-x", "model-1", &meta_a),
            ("provider-x", "model-2", &meta_b),
        ]);
        assert_eq!(written, 2);

        assert_eq!(
            cached_upstream_model_metadata("provider-x", "model-1")
                .unwrap()
                .display_name,
            Some("Model A".into())
        );
        assert_eq!(
            cached_upstream_model_metadata("provider-x", "model-2")
                .unwrap()
                .display_name,
            Some("Model B".into())
        );
    });
}

// Covers: batch metadata writes return 0 and leave no rows when commit fails
// Owner: models.dev cache
#[test]
fn batch_metadata_writes_returns_zero_on_commit_failure() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        {
            let conn = open_models_dev_cache().unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE parent_fk (id INTEGER PRIMARY KEY);
                 DROP TABLE model_metadata;
                 CREATE TABLE model_metadata (
                     provider text not null,
                     model text not null,
                     metadata_json text not null,
                     updated_at integer not null,
                     cache_version integer not null default 1,
                     parent_id integer references parent_fk(id) deferrable initially deferred default 999,
                     primary key (provider, model)
                 );",
            )
            .unwrap();
        }

        let meta = ModelMetadata {
            display_name: Some("Model A".into()),
            advertised_context_window: Some(100_000),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };

        let written =
            write_cached_upstream_model_metadata_batch([("provider-x", "model-1", &meta)]);
        assert_eq!(written, 0, "must report 0 written rows when commit fails");

        assert!(
            cached_upstream_model_metadata("provider-x", "model-1").is_none(),
            "uncommitted rows must not be readable"
        );
    });
}

// Covers: failed catalog hydrate must not advance on-disk snapshot or process readiness
// Owner: models.dev cache
#[test]
fn failed_hydrate_does_not_advance_readiness() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        {
            let conn = open_models_dev_cache().unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE parent_fk (id INTEGER PRIMARY KEY);
                 DROP TABLE model_metadata;
                 CREATE TABLE model_metadata (
                     provider text not null,
                     model text not null,
                     metadata_json text not null,
                     updated_at integer not null,
                     cache_version integer not null default 1,
                     parent_id integer references parent_fk(id) deferrable initially deferred default 999,
                     primary key (provider, model)
                 );",
            )
            .unwrap();
        }

        let api = serde_json::json!({
            "openai": {
                "models": {
                    "gpt-4o": {
                        "name": "GPT-4o",
                        "limit": { "context": 128000, "output": 4096 },
                        "reasoning": false
                    }
                }
            }
        });

        let written = hydrate_catalog_from_api(&api);
        assert_eq!(written, 0);
        assert!(
            !hydrate::catalog_snapshot_is_ready(),
            "snapshot must not be marked ready after failed hydrate"
        );
    });
}

// Covers: models.dev slugs that are not Rho providers still get cache rows
// Owner: models.dev catalog hydrate
#[test]
fn hydrate_writes_non_rho_catalog_slugs() {
    let api = json!({
        "llmgateway": {
            "models": {
                "gpt-5.6-sol": {
                    "name": "GPT-5.6 Sol",
                    "reasoning": false,
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 5.0, "output": 30.0 }
                }
            }
        }
    });

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        assert!(hydrate_catalog_from_api(&api) >= 1);
        let metadata =
            cached_upstream_model_metadata("llmgateway", "gpt-5.6-sol").expect("llmgateway row");
        assert_eq!(metadata.display_name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(metadata.effective_context_window, Some(922000));
        assert_eq!(
            metadata
                .cost_default
                .and_then(|cost| cost.input_micros_per_m),
            Some(5_000_000)
        );
    });
}

// Covers: a custom host with catalog = slug borrows that models.dev row
// Owner: models.dev catalog rematch
#[test]
fn custom_provider_catalog_slug_borrows_upstream_metadata() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    crate::provider::set_custom_provider_catalogs([("cliproxyapi", Some("llmgateway"))]);

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        write_cached_upstream_model_metadata(
            "llmgateway",
            "gpt-5.6-sol",
            &ModelMetadata {
                display_name: Some("GPT-5.6 Sol".into()),
                advertised_context_window: Some(1_050_000),
                effective_context_window: Some(922_000),
                cost_default: Some(ModelCost {
                    input_micros_per_m: Some(5_000_000),
                    output_micros_per_m: Some(30_000_000),
                    ..ModelCost::default()
                }),
                reasoning_metadata_complete: true,
                ..ModelMetadata::default()
            },
        );

        let metadata =
            current_model_metadata("cliproxyapi", "gpt-5.6-sol").expect("borrowed catalog row");
        assert_eq!(metadata.display_name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(metadata.effective_context_window, Some(922_000));
        assert_eq!(
            metadata
                .cost_default
                .and_then(|cost| cost.input_micros_per_m),
            Some(5_000_000)
        );
        assert!(current_model_metadata("cliproxyapi", "missing").is_none());
    });
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
}

// Covers: models.toml catalog remaps provider and optional model id
// Owner: models.dev catalog rematch
#[test]
fn local_catalog_ref_parses_slug_and_qualified_ids() {
    assert_eq!(
        overrides::parse_catalog_ref("llmgateway", "gpt-5.6-sol"),
        Some(("llmgateway".into(), "gpt-5.6-sol".into()))
    );
    assert_eq!(
        overrides::parse_catalog_ref("anthropic/claude-sonnet-4-5", "alias"),
        Some(("anthropic".into(), "claude-sonnet-4-5".into()))
    );
    assert_eq!(
        overrides::parse_catalog_ref("openrouter/anthropic/claude-sonnet-4", "alias"),
        Some(("openrouter".into(), "anthropic/claude-sonnet-4".into()))
    );
    assert_eq!(overrides::parse_catalog_ref("", "gpt-5.6-sol"), None);
    assert_eq!(overrides::parse_catalog_ref("/model", "gpt-5.6-sol"), None);
}

// Covers: inheriting openai-codex applies the built-in window override
// Owner: models.dev catalog rematch
#[test]
fn rematched_openai_codex_catalog_keeps_builtin_window() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    crate::provider::set_custom_provider_catalogs([("cliproxyapi", Some("openai-codex"))]);

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        write_cached_upstream_model_metadata(
            "openai-codex",
            "gpt-5.5",
            &ModelMetadata {
                advertised_context_window: Some(1_050_000),
                effective_context_window: Some(922_000),
                reasoning_metadata_complete: true,
                ..ModelMetadata::default()
            },
        );

        let metadata = current_model_metadata("cliproxyapi", "gpt-5.5").expect("rematched");
        assert_eq!(metadata.effective_context_window, Some(400_000));
    });
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
}
