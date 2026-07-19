use super::*;

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
fn provider_retry_preserves_usage_from_failed_attempt() {
    let mut app = test_app();
    app.model_metadata = Some(input_cost_metadata());
    app.record_agent_event(ViewModelEvent::RunStarted);
    app.record_agent_event(ViewModelEvent::StepStarted(1));
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        ..Default::default()
    }));
    app.record_agent_event(ViewModelEvent::ProviderStreamReset);
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(40),
        ..Default::default()
    }));

    assert_eq!(
        app.cumulative_usage,
        Some(ModelUsage {
            input_tokens: Some(140),
            total_tokens: Some(140),
            cost_usd_micros: Some(140),
            ..Default::default()
        })
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
    app.record_agent_event(ViewModelEvent::ProviderStreamReset);
    app.record_agent_event(ViewModelEvent::Usage(ModelUsage {
        input_tokens: Some(160),
        ..Default::default()
    }));

    assert_eq!(
        app.cumulative_usage,
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
        app.cumulative_usage
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
        app.latest_usage,
        Some(ModelUsage {
            input_tokens: Some(100_000),
            output_tokens: Some(40_000),
            cache_read_tokens: Some(100_000),
            cost_usd_micros: Some(190_000),
            ..ModelUsage::default()
        })
    );
    assert_eq!(
        app.cumulative_usage,
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
        app.cumulative_usage,
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
