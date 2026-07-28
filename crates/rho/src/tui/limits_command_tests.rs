use pretty_assertions::assert_eq;

use super::*;

fn limits_display(view: &LimitsView) -> &LimitsDisplay {
    view.items
        .iter()
        .find_map(|item| match item {
            LimitsViewItem::UsageLimits(limits) => Some(limits),
            _ => None,
        })
        .expect("usage limits block")
}

fn sample_claude_state(observed_at_unix: i64) -> crate::claude_runtime::rate_limit::RateLimitState {
    let mut state = crate::claude_runtime::rate_limit::RateLimitState::default();
    state.merge_window(crate::claude_runtime::rate_limit::RateLimitObservation {
        observed_at_unix,
        observed_seq: 1,
        observed_at_nanos: u64::try_from(observed_at_unix.max(0))
            .unwrap_or(0)
            .saturating_mul(1_000_000_000),
        observed_nonce: "test".into(),
        info: crate::claude_runtime::stream::RateLimitInfo {
            status: Some("allowed".into()),
            rate_limit_type: Some("five_hour".into()),
            resets_at: Some(1_800_000_000),
            utilization: None,
            overage_status: None,
            overage_resets_at: None,
            is_using_overage: Some(false),
        },
    });
    state
}

fn oauth_codex_limits() -> ProviderLimits {
    ProviderLimits {
        providers: vec![ProviderUsageLimits {
            provider: "Codex".into(),
            windows: vec![UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: Some(40.0),
                resets_at_unix: Some(now_unix() + 3_600),
                note: None,
            }],
        }],
    }
}

#[test]
fn running_limits_query_does_not_queue_model_context() {
    let mut app = super::super::tests::test_app();
    app.begin_provider_turn_ui();

    app.render_limits_result(Ok((
        ProviderLimits {
            providers: Vec::new(),
        },
        Vec::new(),
    )));

    assert!(app.pending.steering_prompts().is_empty());
    assert!(app.pending.queued_prompts().is_empty());
    assert!(
        app.history
            .entries()
            .iter()
            .any(|entry| matches!(entry, Entry::UsageLimits(_))),
        "expected a UsageLimits block, got {:?}",
        app.history.entries()
    );
}

#[tokio::test]
async fn cancelling_limits_query_waits_for_background_task_to_stop() {
    let mut app = super::super::tests::test_app();
    let task_marker = std::sync::Arc::new(());
    let captured_marker = task_marker.clone();
    app.pending_usage_limits = Some(tokio::spawn(async move {
        let _marker = captured_marker;
        std::future::pending::<LimitsFetchResult>().await
    }));

    app.cancel_limits_command().await;

    assert!(app.pending_usage_limits.is_none());
    assert_eq!(std::sync::Arc::strong_count(&task_marker), 1);
}

#[test]
fn formats_reset_relative_only_within_one_day() {
    assert_eq!(format_reset_at(200_000, 200_000 - 90 * 60), "in 1h 30m");
    assert!(!format_reset_at(200_000, 0).starts_with("in "));
}

fn claude_provider(display: &LimitsDisplay) -> &ProviderUsageLimits {
    display
        .providers
        .iter()
        .find(|provider| provider.provider == "Claude Code")
        .expect("Claude Code provider")
}

#[test]
fn present_limits_without_claude_cache_states_unknown_even_with_oauth_data() {
    let view = present_limits_result(Ok((oauth_codex_limits(), Vec::new())), None, 2_000);
    let display = limits_display(&view);
    assert!(display
        .providers
        .iter()
        .all(|p| p.provider != "Claude Code"));
    assert_eq!(display.providers.len(), 1);
    assert!(display.empty_note.is_none());
    assert_eq!(view.status, "usage limits updated");
}

#[test]
fn present_limits_with_claude_cache_omits_allowed_and_keeps_age() {
    let state = sample_claude_state(1_000);
    let view = present_limits_result(
        Ok((oauth_codex_limits(), Vec::new())),
        Some(&state),
        1_000 + 125,
    );
    let display = limits_display(&view);
    let claude = claude_provider(display);
    assert_eq!(claude.windows.len(), 1);
    assert_eq!(claude.windows[0].label, "Five hour");
    assert!(
        claude.windows[0]
            .note
            .as_deref()
            .is_some_and(|note| note.contains("observed 2m ago")),
        "{:?}",
        claude.windows[0].note
    );
    assert!(claude.windows[0].remaining_percent.is_none());
}

#[test]
fn present_limits_claude_only_when_no_oauth_providers() {
    let state = sample_claude_state(500);
    let view = present_limits_result(
        Ok((
            ProviderLimits {
                providers: Vec::new(),
            },
            Vec::new(),
        )),
        Some(&state),
        560,
    );
    assert_eq!(view.items.len(), 1);
    let display = limits_display(&view);
    assert_eq!(display.providers.len(), 1);
    assert!(display.empty_note.is_none());
    let claude = claude_provider(display);
    assert_eq!(claude.windows[0].label, "Five hour");
    assert!(claude.windows[0]
        .note
        .as_deref()
        .is_some_and(|note| note.contains("observed 1m ago")));
    assert_eq!(view.status, "claude code limits only");
}

#[test]
fn present_limits_surfaces_utilization_and_warning_status() {
    let mut state = crate::claude_runtime::rate_limit::RateLimitState::default();
    let mut five = sample_claude_state(1_000).windows.remove(0);
    five.info.utilization = Some(0.25);
    five.info.status = Some("allowed_warning".into());
    let mut weekly = five.clone();
    weekly.info.rate_limit_type = Some("seven_day".into());
    weekly.info.utilization = Some(0.4);
    weekly.info.status = Some("allowed".into());
    weekly.observed_at_unix = 1_050;
    weekly.observed_at_nanos = 1_050_000_000_000;
    state.merge_window(five);
    state.merge_window(weekly);

    let view = present_limits_result(Ok((oauth_codex_limits(), Vec::new())), Some(&state), 1_100);
    let display = limits_display(&view);
    let claude = claude_provider(display);
    assert_eq!(claude.windows[0].label, "Five hour");
    assert_eq!(claude.windows[0].remaining_percent, Some(75.0));
    assert!(
        claude.windows[0]
            .note
            .as_deref()
            .is_some_and(|note| note.contains("warning")),
        "{:?}",
        claude.windows[0].note
    );
    assert_eq!(claude.windows[1].label, "Seven day");
    assert_eq!(claude.windows[1].remaining_percent, Some(60.0));
    assert!(
        claude.windows[1]
            .note
            .as_deref()
            .is_some_and(|note| !note.contains("warning") && note.contains("observed")),
        "{:?}",
        claude.windows[1].note
    );
}

#[test]
fn present_limits_never_spawns_or_probes_claude() {
    // Pure helper: injecting None must not invent an observation.
    let view = present_limits_result(
        Err(UsageLimitsError::Unauthorized {
            provider: "Codex",
            login: "/login openai-codex",
        }),
        None,
        0,
    );
    assert!(matches!(
        view.items.as_slice(),
        [LimitsViewItem::Error(error)] if error.contains("Codex")
    ));
    assert_eq!(view.status, "OAuth usage limit check failed");
}
