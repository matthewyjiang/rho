use std::time::Duration;

use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::tui::{app_state::SessionUiPhase, event_adapter::ViewModelEvent, tests::test_app};

fn model_call_metrics() -> rho_sdk::ModelCallMetrics {
    rho_sdk::ModelCallMetrics {
        output_tokens: Some(100),
        time_to_first_token: Some(Duration::from_millis(100)),
        generation_time: Some(Duration::from_millis(1_900)),
        attempt_latency: Duration::from_secs(2),
        total_latency: Duration::from_secs(2),
    }
}

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
fn completed_model_call_updates_the_active_model_average() {
    let mut app = test_app();
    let profile = app.info.runtime.model_call_profile();
    let metrics = model_call_metrics();

    app.record_agent_event(ViewModelEvent::ModelCallCompleted {
        profile: profile.clone(),
        metrics,
    });

    let summary = app.usage.model_performance.summary(&profile);
    assert_eq!(summary.latest_call, Some(metrics));
    assert_eq!(summary.average_output_tokens_per_second, Some(50.0));
    assert_eq!(summary.eligible_calls, 1);
}

#[test]
fn provider_stream_reset_preserves_completed_model_performance() {
    let mut app = test_app();
    let profile = app.info.runtime.model_call_profile();
    app.record_agent_event(ViewModelEvent::ModelCallCompleted {
        profile: profile.clone(),
        metrics: model_call_metrics(),
    });

    app.record_agent_event(ViewModelEvent::ProviderStreamReset);

    let summary = app.usage.model_performance.summary(&profile);
    assert_eq!(summary.average_output_tokens_per_second, Some(50.0));
    assert_eq!(summary.eligible_calls, 1);
}

#[test]
fn step_started_clears_stream_state_without_clearing_model_performance() {
    let mut app = test_app();
    let profile = app.info.runtime.model_call_profile();
    app.streams.assistant_stream.push_delta("current");
    app.streams.reasoning_stream.push_delta("reasoning");
    app.record_agent_event(ViewModelEvent::ModelCallCompleted {
        profile: profile.clone(),
        metrics: model_call_metrics(),
    });

    assert!(app
        .record_agent_event(ViewModelEvent::StepStarted(2))
        .is_none());

    assert!(app.streams.assistant_stream.is_empty());
    assert!(app.streams.reasoning_stream.is_empty());
    let summary = app.usage.model_performance.summary(&profile);
    assert_eq!(summary.average_output_tokens_per_second, Some(50.0));
    assert_eq!(app.turn.session_ui(), SessionUiPhase::ProviderTurn);
    assert_eq!(app.status, "running step 2");
}
