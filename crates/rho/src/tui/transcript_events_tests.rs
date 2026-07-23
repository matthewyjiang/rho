use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::tui::{event_adapter::ViewModelEvent, run_lifecycle::SessionUiPhase, tests::test_app};

#[test]
fn context_usage_event_is_tracked_separately_from_cumulative_usage() {
    let mut app = test_app();
    app.cumulative_usage = Some(ModelUsage {
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
        app.current_context,
        Some(ContextUsage::estimated(250, Some(10_000)))
    );
    assert_eq!(
        app.cumulative_usage
            .as_ref()
            .and_then(|usage| usage.input_tokens),
        Some(1_000)
    );
}

#[test]
fn step_started_clears_stream_state() {
    let mut app = test_app();
    app.assistant_stream.push_delta("current");
    app.reasoning_stream.push_delta("reasoning");

    assert!(app
        .record_agent_event(ViewModelEvent::StepStarted(2))
        .is_none());

    assert!(app.assistant_stream.is_empty());
    assert!(app.reasoning_stream.is_empty());
    assert_eq!(app.session_ui, SessionUiPhase::ProviderTurn);
    assert_eq!(app.status, "running step 2");
}
