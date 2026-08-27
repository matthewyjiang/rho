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

// Covers: catalog cost bills recovered prompt size, including mute turns
// that later merge with a cache-split snapshot; step-start estimates are not this path
// Owner: tui catalog cost estimate
#[test]
fn estimated_cost_bills_recovered_prompt_not_split_remainder() {
    let mute = ModelUsage {
        output_tokens: Some(10),
        total_tokens: Some(1_000_010),
        ..ModelUsage::default()
    };
    let split = ModelUsage {
        input_tokens: Some(300_000),
        cache_read_tokens: Some(700_000),
        output_tokens: Some(100_000),
        total_tokens: Some(1_100_000),
        ..ModelUsage::default()
    };
    let mut mute_then_split = None;
    super::merge_usage(&mut mute_then_split, mute.clone());
    super::merge_usage(&mut mute_then_split, split.clone());

    let cases = [
        (split, Some(570_000)),
        (mute, Some(1_000_020)),
        (
            mute_then_split.expect("merged mute then split"),
            Some(1_570_020),
        ),
    ];
    for (usage, expected) in cases {
        assert_eq!(
            super::estimated_cost_usd_micros(&usage, Some(&priced_metadata())),
            expected
        );
    }
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

// Covers: quiet hosts must show growing output cost until provider usage arrives,
// without restating the prompt as new billed input
// Owner: tui live stream usage estimate
#[test]
fn live_stream_estimate_tracks_output_until_provider_usage() {
    let mut live = super::LiveStreamUsageEstimate::default();
    live.add_output_text("abcd"); // 1 token at 4 chars/token
    live.add_output_text("efghijkl"); // 2 tokens

    assert_eq!(
        live.as_usage(),
        Some(ModelUsage {
            output_tokens: Some(3),
            ..ModelUsage::default()
        })
    );

    let display = super::display_usage_with_live(
        Some(&ModelUsage {
            input_tokens: Some(500),
            output_tokens: Some(20),
            cost_usd_micros: Some(50),
            ..ModelUsage::default()
        }),
        &live,
        Some(&priced_metadata()),
    )
    .expect("display usage");
    assert_eq!(display.input_tokens, Some(500));
    assert_eq!(display.output_tokens, Some(23));
    assert_eq!(display.cost_usd_micros, Some(56));

    live.provider_usage_received();
    assert!(!live.is_active());
    assert_eq!(live.as_usage(), None);
}

// Covers: once a provider reports usage, stream deltas must not invent tokens
// Owner: tui live stream usage estimate
#[test]
fn live_stream_estimate_ignores_deltas_after_provider_usage() {
    let mut live = super::LiveStreamUsageEstimate::default();
    live.add_output_text("abcd");
    live.provider_usage_received();
    live.add_output_text("more output that would otherwise count");
    assert!(!live.is_active());
    assert_eq!(live.as_usage(), None);
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
        super::session_total_cost_usd_micros(Some(570_000), 300_000 + 130_000),
        Some(1_000_000)
    );
    assert_eq!(
        super::session_total_cost_usd_micros(None, 250_000),
        Some(250_000)
    );
    assert_eq!(
        super::session_total_cost_usd_micros(None, 180_000),
        Some(180_000)
    );
    assert_eq!(super::session_total_cost_usd_micros(None, 0), None);
}

// Covers: a zeroed provider-reported window must fall through to the catalog's
// display window instead of hiding it; zero counts as unknown at each stage
// Owner: tui context window resolution
#[test]
fn zeroed_provider_window_falls_back_to_metadata_display_window() {
    use rho_providers::model::ContextUsage;

    let metadata = ModelMetadata {
        advertised_context_window: Some(200_000),
        ..ModelMetadata::default()
    };
    // Provider reports 0: garbage value must not shadow the metadata window.
    assert_eq!(
        super::resolved_context_window(
            Some(&ContextUsage::estimated(1_000, Some(0))),
            Some(&metadata),
        ),
        Some(200_000)
    );
    // A positive provider value still wins over metadata.
    assert_eq!(
        super::resolved_context_window(
            Some(&ContextUsage::estimated(1_000, Some(100_000))),
            Some(&metadata),
        ),
        Some(100_000)
    );
    // Zero everywhere stays unknown.
    assert_eq!(
        super::resolved_context_window(Some(&ContextUsage::estimated(1_000, Some(0))), None,),
        None
    );
}
