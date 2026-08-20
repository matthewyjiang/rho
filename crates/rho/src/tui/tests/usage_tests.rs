use super::*;
use crate::tui::usage_cost::CostSource;
use rho_sdk::model::ContextUsage;

fn input_cost_metadata() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(rho_providers::model::models_dev::ModelCost {
            input_micros_per_m: Some(1_000_000),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn cumulative_cost_source_follows_live_provider_snapshots() {
    let mut app = test_app();
    app.model_metadata = Some(input_cost_metadata());
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        ..Default::default()
    }));
    assert_eq!(
        app.usage.usage_cost_tracker.cumulative_source(),
        CostSource::Estimated
    );

    app.record_agent_event(ViewModelEvent::StepStarted(2));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(200),
        cost_usd_micros: Some(80),
        ..Default::default()
    }));
    assert_eq!(
        app.usage.usage_cost_tracker.cumulative_source(),
        CostSource::ProviderReported
    );
    assert_eq!(
        app.usage
            .cumulative_usage
            .as_ref()
            .and_then(|usage| usage.cost_usd_micros),
        Some(80)
    );
}

#[test]
fn provider_retry_preserves_usage_from_failed_attempt() {
    let mut app = test_app();
    app.model_metadata = Some(input_cost_metadata());
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        ..Default::default()
    }));
    app.record_agent_event(ViewModelEvent::ProviderStreamReset(
        crate::tui::activity::ProviderRetryHint {
            reason: rho_sdk::ProviderStreamResetReason::InvalidResponse,
        },
    ));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(40),
        cost_usd_micros: Some(40),
        ..Default::default()
    }));

    assert_eq!(
        app.usage.cumulative_usage,
        Some(ModelUsage {
            input_tokens: Some(140),
            total_tokens: Some(140),
            cost_usd_micros: Some(140),
            ..Default::default()
        })
    );
    assert_eq!(
        app.usage.usage_cost_tracker.cumulative_source(),
        CostSource::Estimated
    );

    app.record_agent_event(ViewModelEvent::StepStarted(2));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(50),
        cost_usd_micros: Some(50),
        ..Default::default()
    }));
    assert_eq!(
        app.usage.usage_cost_tracker.cumulative_source(),
        CostSource::Estimated
    );
}

#[test]
fn provider_retry_after_prior_step_does_not_double_count_completed_usage() {
    let mut app = test_app();
    app.model_metadata = Some(input_cost_metadata());
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        ..Default::default()
    }));
    app.record_agent_event(ViewModelEvent::StepStarted(2));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(140),
        ..Default::default()
    }));
    app.record_agent_event(ViewModelEvent::ProviderStreamReset(
        crate::tui::activity::ProviderRetryHint {
            reason: rho_sdk::ProviderStreamResetReason::InvalidResponse,
        },
    ));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(160),
        ..Default::default()
    }));

    assert_eq!(
        app.usage.cumulative_usage,
        Some(ModelUsage {
            input_tokens: Some(200),
            total_tokens: Some(200),
            cost_usd_micros: Some(200),
            ..Default::default()
        })
    );
}

#[test]
fn metadata_loaded_after_first_step_recomputes_uncosted_baseline() {
    let mut app = test_app();
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        ..Default::default()
    }));
    app.model_metadata = Some(input_cost_metadata());
    app.record_agent_event(ViewModelEvent::StepStarted(2));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(140),
        ..Default::default()
    }));

    assert_eq!(
        app.usage
            .cumulative_usage
            .as_ref()
            .and_then(|usage| usage.cost_usd_micros),
        Some(140)
    );
}

#[test]
fn cumulative_usage_replaces_live_run_snapshots_and_adds_completed_runs() {
    let mut app = test_app();
    app.model_metadata = Some(ModelMetadata {
        cost_default: Some(rho_providers::model::models_dev::ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(2_000_000),
            cache_read_micros_per_m: Some(100_000),
            cache_write_micros_per_m: None,
        }),
        long_context_threshold: Some(200_000),
        cost_long_context: Some(rho_providers::model::models_dev::ModelCost {
            input_micros_per_m: Some(4_000_000),
            output_micros_per_m: Some(8_000_000),
            cache_read_micros_per_m: Some(400_000),
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    });

    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(100_000),
        output_tokens: Some(20_000),
        cache_read_tokens: Some(50_000),
        ..ModelUsage::default()
    }));
    app.record_agent_event(ViewModelEvent::StepStarted(2));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(200_000),
        output_tokens: Some(60_000),
        cache_read_tokens: Some(150_000),
        ..ModelUsage::default()
    }));

    assert_eq!(
        app.usage.latest_usage,
        Some(ModelUsage {
            input_tokens: Some(100_000),
            output_tokens: Some(40_000),
            cache_read_tokens: Some(100_000),
            cost_usd_micros: Some(190_000),
            ..ModelUsage::default()
        })
    );
    assert_eq!(
        app.usage.cumulative_usage,
        Some(ModelUsage {
            input_tokens: Some(200_000),
            output_tokens: Some(60_000),
            cache_read_tokens: Some(150_000),
            total_tokens: Some(410_000),
            cost_usd_micros: Some(335_000),
            ..ModelUsage::default()
        })
    );

    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(10_000),
        output_tokens: Some(5_000),
        cache_read_tokens: Some(90_000),
        ..ModelUsage::default()
    }));

    assert_eq!(
        app.usage.cumulative_usage,
        Some(ModelUsage {
            input_tokens: Some(210_000),
            output_tokens: Some(65_000),
            cache_read_tokens: Some(240_000),
            total_tokens: Some(515_000),
            cost_usd_micros: Some(364_000),
            ..ModelUsage::default()
        })
    );
}

// Covers: quiet hosts must not reprice estimated context on submit; live cost
// grows from streamed output, then yields to provider-reported usage
// Owner: tui transcript usage accounting
#[test]
fn live_stream_estimate_grows_during_reasoning_and_yields_to_provider() {
    let mut app = test_app();
    app.model_metadata = Some(ModelMetadata {
        cost_default: Some(rho_providers::model::models_dev::ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(2_000_000),
            cache_read_micros_per_m: None,
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    });
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(80),
        output_tokens: Some(10),
        cost_usd_micros: Some(100),
        ..Default::default()
    }));
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::ContextUsage(ContextUsage::estimated(
        1_000,
        Some(10_000),
    )));
    assert!(!app.usage.live_stream.is_active());
    assert_eq!(
        crate::tui::usage_cost::display_usage_with_live(
            app.usage.cumulative_usage.as_ref(),
            &app.usage.live_stream,
            app.model_metadata.as_ref(),
        ),
        Some(ModelUsage {
            input_tokens: Some(80),
            output_tokens: Some(10),
            total_tokens: Some(90),
            cost_usd_micros: Some(100),
            ..Default::default()
        })
    );

    app.record_agent_event(ViewModelEvent::LiveOutputText(
        "a".repeat(16), // 16 chars => 4 tokens
    ));
    assert!(app.usage.live_stream.is_active());
    let display = crate::tui::usage_cost::display_usage_with_live(
        app.usage.cumulative_usage.as_ref(),
        &app.usage.live_stream,
        app.model_metadata.as_ref(),
    )
    .expect("live display usage");
    assert_eq!(display.input_tokens, Some(80));
    assert_eq!(display.output_tokens, Some(14));
    assert_eq!(display.cost_usd_micros, Some(108));

    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(1_000),
        output_tokens: Some(10),
        cost_usd_micros: Some(1_020),
        ..Default::default()
    }));
    assert!(!app.usage.live_stream.is_active());
    assert_eq!(
        app.usage.cumulative_usage,
        Some(ModelUsage {
            input_tokens: Some(1_080),
            output_tokens: Some(20),
            total_tokens: Some(1_100),
            cost_usd_micros: Some(1_120),
            ..Default::default()
        })
    );
}

// Covers: ending a busy turn must drop live estimates so statusline cost is ledger + subagents
// Owner: tui run lifecycle usage accounting
#[test]
fn end_busy_ui_clears_live_stream_estimate() {
    let mut app = test_app();
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::ContextUsage(ContextUsage::estimated(
        2_500,
        Some(10_000),
    )));
    app.record_agent_event(ViewModelEvent::LiveOutputText("abcd".into()));
    assert!(app.usage.live_stream.is_active());

    app.end_busy_ui();

    assert!(!app.usage.live_stream.is_active());
}
