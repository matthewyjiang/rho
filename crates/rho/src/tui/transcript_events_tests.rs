use std::time::Duration;

use rho_sdk::model::{ContextUsage, ModelUsage};
use rho_sdk::ProviderStreamResetReason;

use crate::tui::{
    activity::ProviderRetryHint,
    app_state::SessionUiPhase,
    cache_stats::CacheRebilled,
    compaction_display::CompactionUiOutcome,
    event_adapter::{compact_finished_event, ViewModelEvent},
    model_performance::GenerationOutputTokens,
    tests::test_app,
    App,
};

fn model_call_metrics() -> rho_sdk::ModelCallMetrics {
    rho_sdk::ModelCallMetrics {
        output_tokens: Some(140),
        time_to_first_token: Some(Duration::from_millis(100)),
        generation_time: Some(Duration::from_millis(1_900)),
        total_latency: Duration::from_secs(2),
        generation_output_tokens: None,
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
        generation_output_tokens: GenerationOutputTokens::Reported(100),
    });

    let summary = app.usage.model_performance.summary(&profile);
    assert_eq!(summary.latest_call.map(|call| call.metrics), Some(metrics));
    assert_eq!(
        summary
            .latest_call
            .map(|call| call.generation_output_tokens),
        Some(GenerationOutputTokens::Reported(100))
    );
    assert_eq!(
        summary.average_generation_tokens_per_second,
        Some(100.0 / 1.9)
    );
    assert_eq!(summary.eligible_calls, 1);
}

#[test]
fn provider_stream_reset_preserves_completed_model_performance() {
    let mut app = test_app();
    let profile = app.info.runtime.model_call_profile();
    app.record_agent_event(ViewModelEvent::ModelCallCompleted {
        profile: profile.clone(),
        metrics: model_call_metrics(),
        generation_output_tokens: GenerationOutputTokens::Reported(100),
    });

    app.record_agent_event(ViewModelEvent::ProviderStreamReset(ProviderRetryHint {
        reason: ProviderStreamResetReason::InvalidResponse,
    }));

    let summary = app.usage.model_performance.summary(&profile);
    assert_eq!(
        summary.average_generation_tokens_per_second,
        Some(100.0 / 1.9)
    );
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
        generation_output_tokens: GenerationOutputTokens::Reported(100),
    });

    assert!(app
        .record_agent_event(ViewModelEvent::StepStarted(2))
        .is_none());

    assert!(app.streams.assistant_stream.is_empty());
    assert!(app.streams.reasoning_stream.is_empty());
    let summary = app.usage.model_performance.summary(&profile);
    assert_eq!(
        summary.average_generation_tokens_per_second,
        Some(100.0 / 1.9)
    );
    assert_eq!(app.turn.session_ui(), SessionUiPhase::ProviderTurn);
    assert_eq!(app.status(), "running step 2");
}

#[test]
fn provider_retry_status_includes_rate_limit_reset_hint() {
    use rho_sdk::ProviderErrorKind;

    assert_eq!(
        ProviderRetryHint {
            reason: ProviderStreamResetReason::RetryableFailure {
                kind: ProviderErrorKind::RateLimit,
                retry_after: Some(Duration::from_secs(12)),
            },
        }
        .status_label(),
        "rate limited · retry in 12s"
    );
    assert_eq!(
        ProviderRetryHint {
            reason: ProviderStreamResetReason::RetryableFailure {
                kind: ProviderErrorKind::RateLimit,
                retry_after: None,
            },
        }
        .status_label(),
        "rate limited · retrying"
    );
    assert_eq!(
        ProviderRetryHint {
            reason: ProviderStreamResetReason::InvalidResponse,
        }
        .status_label(),
        "retrying provider response"
    );
}

fn complete_model_call(app: &mut App) {
    app.record_agent_event(ViewModelEvent::ModelCallCompleted {
        profile: app.info.runtime.model_call_profile(),
        metrics: model_call_metrics(),
        generation_output_tokens: GenerationOutputTokens::Reported(1),
    });
}

fn report_step_usage(app: &mut App, step: usize, usage: ModelUsage) {
    app.record_agent_event(ViewModelEvent::StepStarted(step));
    app.record_agent_event(ViewModelEvent::Usage(usage));
    complete_model_call(app);
}

// Covers: the event path feeds the tracker a per-step delta, not a
// cumulative snapshot, and compaction resets the prefix before the next call.
// Owner: tui transcript events
#[test]
fn cache_stats_count_a_miss_from_usage_then_model_call_completed() {
    let mut app = test_app();
    app.record_agent_event(ViewModelEvent::RunStarted);
    // Usage events are cumulative within the run. Step 1 establishes a 50K
    // prefix; step 2's snapshot still includes that write plus a larger
    // cache-read total. The per-step delta is a 20K miss; the raw snapshot
    // would look like a full hit.
    report_step_usage(
        &mut app,
        1,
        ModelUsage {
            cache_read_tokens: Some(40_000),
            cache_write_tokens: Some(10_000),
            ..ModelUsage::default()
        },
    );
    report_step_usage(
        &mut app,
        2,
        ModelUsage {
            input_tokens: Some(20_000),
            cache_read_tokens: Some(60_000),
            cache_write_tokens: Some(10_000),
            ..ModelUsage::default()
        },
    );

    assert_eq!(
        app.usage.cache_stats.rebilled(),
        &CacheRebilled {
            missed_tokens: 20_000,
            miss_count: 1,
            extra_cost_usd_micros: 0,
            unpriced_miss_count: 1,
        }
    );

    app.record_agent_event(compact_finished_event(CompactionUiOutcome::unchanged()));
    report_step_usage(
        &mut app,
        3,
        ModelUsage {
            input_tokens: Some(80_000),
            cache_read_tokens: Some(60_000),
            cache_write_tokens: Some(10_000),
            ..ModelUsage::default()
        },
    );

    assert_eq!(app.usage.cache_stats.rebilled().miss_count, 1);
    assert_eq!(app.usage.cache_stats.take_turn_notices().len(), 1);
}

// Covers: a later generated image must not attach to an earlier unfilled card.
// Owner: tui transcript events
#[test]
fn insert_assistant_images_appends_a_card_per_image() {
    use rho_providers::model::ImageContent;
    use rho_sdk::model::ContentBlock;

    let first = ImageContent {
        data: "aW1n".into(),
        mime_type: "image/png".into(),
    };
    let second = ImageContent {
        data: "aW1nMg==".into(),
        mime_type: "image/png".into(),
    };
    let mut app = test_app();
    app.history
        .set_entries(vec![crate::tui::message_history::generated_image_entry(
            Ok(None),
            &first,
        )]);
    app.insert_assistant_images(&[ContentBlock::Image(second)]);
    assert_eq!(app.history.entries().len(), 2);
    assert!(app
        .history
        .entries()
        .iter()
        .all(|entry| matches!(entry, crate::tui::Entry::Tool(tool) if tool.image.is_none())));
}

// Covers: a completed turn stamps duration on this turn's assistant row, not
// an earlier reply or a later notice.
// Owner: pure unit (turn receipt attachment)
#[test]
fn attach_turn_worked_stamps_current_turn_assistant() {
    let mut app = test_app();
    app.push_transcript_entry(crate::tui::Entry::Assistant("previous".into()));
    app.push_transcript_entry(crate::tui::Entry::User("prompt".into()));
    app.turn.set_current_turn_start(Some(app.history.len()));
    app.push_transcript_entry(crate::tui::Entry::Assistant("answer".into()));
    app.push_transcript_entry(crate::tui::Entry::Notice("after".into()));

    app.attach_turn_worked(Duration::from_secs(15));

    assert!(matches!(
        app.history.entries(),
        [
            crate::tui::Entry::Assistant(previous),
            crate::tui::Entry::User(_),
            crate::tui::Entry::Assistant(answer),
            crate::tui::Entry::Notice(_)
        ] if previous.worked_for.is_none()
            && answer.text == "answer"
            && answer.worked_for == Some(Duration::from_secs(15))
    ));
}

// Covers: a completed tool-only turn still gets a duration receipt.
// Owner: pure unit (turn receipt attachment)
#[test]
fn attach_turn_worked_inserts_summary_when_turn_has_no_assistant() {
    let mut app = test_app();
    app.push_transcript_entry(crate::tui::Entry::User("prompt".into()));
    app.turn.set_current_turn_start(Some(app.history.len()));

    app.attach_turn_worked(Duration::from_millis(1_500));

    assert!(matches!(
        app.history.entries(),
        [crate::tui::Entry::User(_), crate::tui::Entry::Assistant(assistant)]
            if assistant.text.is_empty()
                && assistant.worked_for == Some(Duration::from_millis(1_500))
    ));
}

// Covers: a missing turn start never stamps earlier history.
// Owner: pure unit (turn receipt attachment)
#[test]
fn attach_turn_worked_without_turn_start_inserts_summary() {
    let mut app = test_app();
    app.push_transcript_entry(crate::tui::Entry::Assistant("previous".into()));

    app.attach_turn_worked(Duration::from_secs(4));

    assert!(matches!(
        app.history.entries(),
        [
            crate::tui::Entry::Assistant(previous),
            crate::tui::Entry::Assistant(summary)
        ] if previous.text == "previous"
            && previous.worked_for.is_none()
            && summary.text.is_empty()
            && summary.worked_for == Some(Duration::from_secs(4))
    ));
}
