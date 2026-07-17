use super::*;
use crate::model::provider_models::{
    replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
    ProviderModel,
};
use pretty_assertions::assert_eq;
use serde_json::json;

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
fn effort_options_without_none_still_support_off_by_omission() {
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
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ])
    );
}

#[test]
fn unknown_effort_values_do_not_restrict_reasoning() {
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

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
}

#[test]
fn models_without_effort_choices_only_expose_off() {
    let api = serde_json::json!({
        "openai": {
            "models": {
                "gpt-test": {"reasoning": true, "reasoning_options": []}
            }
        }
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Off])
    );
}

#[test]
fn leaves_unknown_reasoning_option_schemas_unrestricted() {
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

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(metadata.supported_reasoning_levels, None);
}

#[test]
fn non_reasoning_models_only_support_off() {
    let api = serde_json::json!({
        "openai": {"models": {"gpt-test": {"reasoning": false}}}
    });

    let metadata = model_metadata_from_api(&api, "openai", "gpt-test").unwrap();

    assert!(metadata.reasoning_capabilities_known);
    assert_eq!(
        metadata.supported_reasoning_levels,
        Some(vec![ReasoningLevel::Off])
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
fn builtin_gpt_56_codex_overrides_match_upstream_catalog() {
    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let metadata = apply_builtin_overrides("openai-codex", model, ModelMetadata::default());

        assert_eq!(metadata.effective_context_window, Some(372_000));
        assert_eq!(metadata.usable_context_window, Some(372_000));
        assert_eq!(metadata.display_context_window(), Some(372_000));
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
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
            ]),
            reasoning_off_behavior: ReasoningOffBehavior::Omit,
            reasoning_capabilities_known: true,
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
fn rehydrates_when_cache_version_is_stale_or_capabilities_are_unknown() {
    let complete = ModelMetadata {
        supported_reasoning_levels: Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]),
        reasoning_capabilities_known: true,
        ..ModelMetadata::default()
    };
    let missing_flag = ModelMetadata {
        supported_reasoning_levels: Some(vec![ReasoningLevel::Off, ReasoningLevel::High]),
        reasoning_capabilities_known: false,
        ..ModelMetadata::default()
    };
    let intentional_unrestricted = ModelMetadata {
        supported_reasoning_levels: None,
        reasoning_capabilities_known: true,
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
    // Intentional unrestricted schemes (budget tokens, unknown efforts) seal
    // with known=true and levels=None; those must not thrash on rehydrate.
    assert!(!should_rehydrate_cached_metadata(
        MODEL_METADATA_CACHE_VERSION,
        &intentional_unrestricted
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
