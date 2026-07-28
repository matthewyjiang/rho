use std::time::Duration;

use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::tui::{app_state::SessionUiPhase, event_adapter::ViewModelEvent, tests::test_app};

#[test]
fn context_usage_event_is_tracked_separately_from_cumulative_usage() {
    let mut app = test_app();
    app.usage.cumulative_usage = Some(ModelUsage {
        input_tokens: Some(1_000),
        output_tokens: Some(500),
        ..ModelUsage::default()
    });

    assert!(app
        .record_agent_event(ViewModelEvent::ContextUsage(ContextUsage::estimated(
            250,
            Some(10_000),
        )))
        .is_none());

    assert_eq!(
        app.usage.current_context,
        Some(ContextUsage::estimated(250, Some(10_000)))
    );
    assert_eq!(
        app.usage
            .cumulative_usage
            .as_ref()
            .and_then(|usage| usage.input_tokens),
        Some(1_000)
    );
}

#[test]
fn provider_stream_reset_clears_stale_model_call_metrics() {
    let mut app = test_app();
    app.usage.latest_model_call = Some(rho_sdk::ModelCallMetrics::new(
        /*output_tokens*/ Some(10),
        /*time_to_first_token*/ Some(Duration::from_millis(100)),
        /*generation_time*/ Some(Duration::from_secs(1)),
        /*total_latency*/ Duration::from_millis(1_100),
    ));

    app.reset_provider_attempt_stream();

    assert_eq!(app.usage.latest_model_call, None);
}

#[test]
fn step_started_clears_stream_state() {
    let mut app = test_app();
    app.streams.assistant_stream.push_delta("current");
    app.streams.reasoning_stream.push_delta("reasoning");

    assert!(app
        .record_agent_event(ViewModelEvent::StepStarted(2))
        .is_none());

    assert!(app.streams.assistant_stream.is_empty());
    assert!(app.streams.reasoning_stream.is_empty());
    assert_eq!(app.turn.session_ui(), SessionUiPhase::ProviderTurn);
    assert_eq!(app.status, "running step 2");
}
