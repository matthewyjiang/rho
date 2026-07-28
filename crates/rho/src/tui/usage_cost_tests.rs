use rho_providers::model::{models_dev::ModelCost, ModelMetadata, ModelUsage};

fn priced_metadata() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(2_000_000),
            cache_read_micros_per_m: Some(100_000),
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    }
}

#[test]
fn estimated_cost_uses_normalized_input_and_cache_read() {
    let usage = ModelUsage {
        input_tokens: Some(300_000),
        cache_read_tokens: Some(700_000),
        output_tokens: Some(100_000),
        ..ModelUsage::default()
    };

    assert_eq!(
        super::estimated_cost_usd_micros(&usage, Some(&priced_metadata())),
        Some(570_000)
    );
}

#[test]
fn cost_tracker_replaces_live_snapshots_but_keeps_retry_estimates() {
    let reported = ModelUsage {
        cost_usd_micros: Some(10),
        ..ModelUsage::default()
    };
    let estimated = ModelUsage::default();
    let mut tracker = super::UsageCostTracker::default();

    tracker.run_started();
    tracker.step_started();
    tracker.record_usage(&estimated);
    assert_eq!(tracker.cumulative_source(), super::CostSource::Estimated);

    tracker.record_usage(&reported);
    assert_eq!(
        tracker.cumulative_source(),
        super::CostSource::ProviderReported
    );

    tracker.record_usage(&estimated);
    tracker.attempt_restarted();
    tracker.record_usage(&reported);
    assert_eq!(tracker.cumulative_source(), super::CostSource::Estimated);

    tracker.step_started();
    tracker.record_usage(&reported);
    assert_eq!(tracker.cumulative_source(), super::CostSource::Estimated);
}

#[test]
fn cost_tracker_preserves_estimates_from_completed_runs() {
    let mut tracker = super::UsageCostTracker::default();
    tracker.run_started();
    tracker.step_started();
    tracker.record_usage(&ModelUsage::default());

    tracker.run_started();
    tracker.step_started();
    tracker.record_usage(&ModelUsage {
        cost_usd_micros: Some(10),
        ..ModelUsage::default()
    });

    assert_eq!(tracker.cumulative_source(), super::CostSource::Estimated);
}

#[test]
fn attempt_aware_run_usage_preserves_failed_attempt_tokens() {
    let mut usage = super::AttemptAwareRunUsage::default();
    usage.step_started();
    usage.apply_snapshot(
        ModelUsage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            cache_read_tokens: Some(50),
            ..ModelUsage::default()
        },
        |snapshot| snapshot,
    );
    usage.attempt_reset();
    usage.apply_snapshot(
        ModelUsage {
            input_tokens: Some(40),
            output_tokens: Some(4),
            cache_read_tokens: Some(20),
            ..ModelUsage::default()
        },
        |snapshot| snapshot,
    );

    assert_eq!(
        usage.current(),
        Some(&ModelUsage {
            input_tokens: Some(140),
            output_tokens: Some(14),
            cache_read_tokens: Some(70),
            total_tokens: Some(224),
            ..ModelUsage::default()
        })
    );
}

#[test]
fn resolves_and_combines_session_costs() {
    let usage = ModelUsage {
        cost_usd_micros: Some(570_000),
        ..ModelUsage::default()
    };
    assert_eq!(
        super::resolved_usage_cost_usd_micros(&usage, Some(&priced_metadata())),
        Some(570_000)
    );
    assert_eq!(
        super::session_total_cost_usd_micros(Some(570_000), 430_000),
        Some(1_000_000)
    );
    assert_eq!(
        super::session_total_cost_usd_micros(None, 250_000),
        Some(250_000)
    );
    assert_eq!(super::session_total_cost_usd_micros(None, 0), None);
}
