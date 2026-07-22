use super::*;
use crate::model::{
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ReasoningLevelSet,
};
use pretty_assertions::assert_eq;
use serde_json::json;

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
fn openrouter_resolves_models_dev_identity_from_model_prefix() {
    let api = json!({
        "anthropic": {
            "models": {
                "claude-sonnet-4": {
                    "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["low", "high"]}],
                    "limit": {"context": 200_000, "output": 64_000}
                }
            }
        }
    });

    let metadata =
        upstream_metadata_from_api(&api, "openrouter", "anthropic/claude-sonnet-4").unwrap();

    assert_eq!(metadata.advertised_context_window, Some(200_000));
    assert_eq!(metadata.max_output_tokens, Some(64_000));
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High
        ])
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

#[test]
fn poolside_maps_reasoning_without_effort_options_to_off_or_max() {
    let api = json!({
        "poolside": {
            "models": {
                "poolside/laguna-s-2.1": {
                    "reasoning": true,
                    "reasoning_options": [],
                    "limit": {"context": 262_144, "output": 32_768}
                }
            }
        }
    });

    let metadata = upstream_metadata_from_api(&api, "poolside", "laguna-s-2.1").unwrap();

    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Off, ReasoningLevel::Max])
    );
    assert!(metadata.reasoning_capabilities_known);
    assert!(metadata.reasoning_metadata_complete);
    assert_eq!(
        current_reasoning_capabilities("poolside", "laguna-s-2.1"),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Max,
        ]))
    );
}

#[test]
fn kimi_code_resolves_k3_models_dev_identity() {
    let api = json!({
        "moonshotai": {
            "models": {
                "kimi-k3": {
                    "reasoning": true,
                    "reasoning_options": [
                        {"type": "toggle"},
                        {"type": "effort", "values": ["max"]}
                    ],
                    "limit": {"context": 1_048_576, "output": 131_072}
                }
            }
        }
    });

    let metadata = upstream_metadata_from_api(&api, "kimi-code", "k3").unwrap();

    assert_eq!(metadata.advertised_context_window, Some(1_048_576));
    assert_eq!(metadata.effective_context_window, Some(1_048_576));
    assert_eq!(metadata.max_output_tokens, Some(131_072));
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Off, ReasoningLevel::Max])
    );
    assert!(metadata.reasoning_capabilities_known);
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
fn non_configurable_provider_path_is_known_without_model_metadata() {
    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir(cache.path().to_path_buf(), || {
        assert_eq!(
            cached_reasoning_capabilities("github-copilot", "unseen-model"),
            ReasoningCapabilities::NotConfigurable
        );
    });
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
fn unknown_effort_values_leave_capabilities_unknown() {
    let api = serde_json::json!({
        "openai": {
            "models": {
                "gpt-test": {
                    "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["default"]}]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert!(!metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
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
fn xai_catalog_levels_are_interpreted_exactly() {
    let api = json!({
        "xai": {
            "models": {
                "grok-test": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "medium", "high"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "xai", "grok-test").unwrap();
    let levels = metadata.supported_reasoning_levels.unwrap();
    assert_eq!(
        levels,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]
    );
    assert_eq!(
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(levels))
            .next_level(ReasoningLevel::High),
        ReasoningLevel::Low
    );
}

#[test]
fn builtin_gpt_56_codex_overrides_use_safer_effective_windows() {
    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let metadata = apply_builtin_overrides("openai-codex", model, ModelMetadata::default());

        assert_eq!(metadata.effective_context_window, Some(272_000));
        assert_eq!(metadata.usable_context_window, Some(272_000));
        assert_eq!(metadata.display_context_window(), Some(272_000));
        assert_eq!(metadata.supported_reasoning_levels, None);
        assert!(!metadata.reasoning_capabilities_known);
    }
}

#[test]
fn builtin_gpt_55_overrides_use_safer_effective_windows() {
    let upstream = ModelMetadata {
        advertised_context_window: Some(1_050_000),
        effective_context_window: Some(922_000),
        max_output_tokens: Some(128_000),
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(5_000_000),
            output_micros_per_m: Some(30_000_000),
            cache_read_micros_per_m: Some(500_000),
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    };
    let openai = apply_builtin_overrides("openai", "gpt-5.5", upstream.clone());
    let codex = apply_builtin_overrides("openai-codex", "gpt-5.5", upstream);

    assert_eq!(openai.display_context_window(), Some(272_000));
    assert_eq!(openai.effective_context_window, Some(922_000));
    assert_eq!(codex.display_context_window(), Some(272_000));
    assert_eq!(codex.effective_context_window, Some(400_000));
    assert_eq!(codex.advertised_context_window, Some(1_050_000));
    assert_eq!(codex.long_context_threshold, Some(272_000));
    assert_eq!(codex.max_output_tokens, Some(128_000));
    assert_eq!(
        codex.cost_default.unwrap().input_micros_per_m,
        Some(5_000_000)
    );
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
                        "context_over_200k": {
                            "input": 4.0,
                            "output": 12.0,
                            "cache_read": 1.0
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
fn codex_models_skip_minimal_when_models_dev_omits_it() {
    let api = json!({
        "openai": {
            "models": {
                "gpt-5.3-codex": {
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["none", "low", "medium", "high", "xhigh"]
                    }]
                }
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-5.3-codex").unwrap();

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ])
    );
    assert_eq!(
        metadata.reasoning_off_behavior,
        ReasoningOffBehavior::EffortNone
    );
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
fn unknown_provider_capabilities_keep_catalog_fallback() {
    let metadata = apply_provider_capabilities(
        "missing-provider",
        "missing-model",
        ModelMetadata {
            supported_reasoning_levels: Some(vec![ReasoningLevel::Off, ReasoningLevel::Max]),
            reasoning_capabilities_known: true,
            ..ModelMetadata::default()
        },
    );

    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Off, ReasoningLevel::Max])
    );
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

    let metadata = merge_toml_override(provider_metadata, table.as_table().unwrap());

    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Medium, ReasoningLevel::Xhigh])
    );
    assert!(metadata.reasoning_capabilities_known);
}

#[test]
fn context_only_override_does_not_hide_unknown_reasoning_capabilities() {
    let metadata = ModelMetadata {
        effective_context_window: Some(262_144),
        ..ModelMetadata::default()
    };

    assert!(metadata_has_values(&metadata));
    assert_eq!(
        ReasoningCapabilities::from_metadata(
            metadata.supported_reasoning_levels,
            metadata.reasoning_capabilities_known,
        ),
        ReasoningCapabilities::Unknown
    );
}
