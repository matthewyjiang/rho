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
fn formats_usd_for_compact_display() {
    assert_eq!(super::format_usd(570_000), "$0.570");
    assert_eq!(super::format_usd(12_340_000), "$12.34");
    assert_eq!(super::format_usd(123_400_000), "$123");
}
