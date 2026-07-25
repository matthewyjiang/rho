use pretty_assertions::assert_eq;

use super::*;

fn line_text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn display_from_providers(providers: Vec<ProviderUsageLimits>) -> LimitsDisplay {
    LimitsDisplay {
        providers,
        empty_note: None,
    }
}

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
fn renders_only_available_windows_with_remaining_bar() {
    let lines = usage_limit_lines(
        &display_from_providers(vec![ProviderUsageLimits {
            provider: "Codex".into(),
            windows: vec![UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: Some(69.0),
                resets_at_unix: Some(now_unix() + 2 * 60 * 60 + 14 * 60),
                note: None,
            }],
        }]),
        80,
    );
    let text = line_text(&lines);

    assert_eq!(text[0].trim_end(), "Usage limits");
    assert_eq!(text[2].trim_end(), "Codex");
    assert!(text[3].contains("Weekly"));
    assert!(text[3].contains("███████░░░"));
    assert!(text[3].contains("69% left"));
    assert!(text[3].contains("resets in 2h 14m"));
    assert!(!text.join("\n").contains("5-hour"));
    assert!(!text.iter().any(|line| line.trim_end() == "Claude Code"));
    assert!(lines.iter().all(|line| line.width() == 80));
    assert!(lines.iter().all(|line| {
        line.style.bg.is_some() || line.spans.iter().all(|span| span.style.bg.is_some())
    }));
}

#[test]
fn renders_multiple_connected_providers() {
    let lines = usage_limit_lines(
        &display_from_providers(vec![
            ProviderUsageLimits {
                provider: "Codex".into(),
                windows: vec![UsageLimitWindow {
                    label: "Weekly".into(),
                    remaining_percent: Some(69.0),
                    resets_at_unix: Some(now_unix() + 2 * 60 * 60 + 14 * 60),
                    note: None,
                }],
            },
            ProviderUsageLimits {
                provider: "xAI".into(),
                windows: vec![UsageLimitWindow {
                    label: "Weekly".into(),
                    remaining_percent: Some(97.0),
                    resets_at_unix: Some(now_unix() + 3 * 24 * 60 * 60),
                    note: None,
                }],
            },
        ]),
        80,
    );
    let text = line_text(&lines);

    assert_eq!(text[2].trim_end(), "Codex");
    assert!(text.iter().any(|line| line.trim_end() == "xAI"));
    assert!(text.iter().any(|line| line.contains("97% left")));
    assert!(!text.iter().any(|line| line.trim_end() == "Claude Code"));
}

#[test]
fn renders_claude_windows_inside_the_command_block() {
    let lines = usage_limit_lines(
        &LimitsDisplay {
            providers: vec![
                ProviderUsageLimits {
                    provider: "Codex".into(),
                    windows: vec![UsageLimitWindow {
                        label: "Weekly".into(),
                        remaining_percent: Some(40.0),
                        resets_at_unix: Some(now_unix() + 3_600),
                        note: None,
                    }],
                },
                ProviderUsageLimits {
                    provider: "Claude Code".into(),
                    windows: vec![
                        UsageLimitWindow {
                            label: "Five hour".into(),
                            remaining_percent: Some(69.0),
                            resets_at_unix: Some(now_unix() + 2 * 60 * 60 + 14 * 60),
                            note: Some("observed 2m ago".into()),
                        },
                        UsageLimitWindow {
                            label: "Seven day".into(),
                            remaining_percent: None,
                            resets_at_unix: Some(now_unix() + 3 * 24 * 60 * 60),
                            note: Some("observed 2m ago".into()),
                        },
                    ],
                },
            ],
            empty_note: None,
        },
        80,
    );
    let text = line_text(&lines);
    let claude_idx = text
        .iter()
        .position(|line| line.trim_end() == "Claude Code")
        .expect("Claude Code section");
    assert!(text[claude_idx + 1].contains("Five hour"), "{text:?}");
    assert!(text[claude_idx + 1].contains("69% left"), "{text:?}");
    assert!(text[claude_idx + 1].contains("███████░░░"), "{text:?}");
    assert!(text[claude_idx + 2].contains("Seven day"), "{text:?}");
    assert!(!text[claude_idx + 2].contains('%'), "{text:?}");
    assert!(text.iter().any(|line| line.contains("observed 2m ago")));
    assert!(!text.join("\n").contains("allowed"));
    assert!(lines.iter().all(|line| {
        line.style.bg.is_some() || line.spans.iter().all(|span| span.style.bg.is_some())
    }));
}

#[test]
fn narrow_layout_wraps_reset_instead_of_hiding_it() {
    let lines = usage_limit_window_lines(
        &UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: Some(93.0),
            resets_at_unix: Some(10_000),
            note: None,
        },
        6,
        43,
        10_000 - 2 * 60 * 60 - 14 * 60,
        Theme::command_block(),
    );
    let text = line_text(&lines);

    assert_eq!(
        text,
        vec![
            "  Weekly   █████████░  93% left".to_string(),
            "  resets in 2h 14m".to_string(),
        ]
    );
}

#[test]
fn formats_reset_relative_only_within_one_day() {
    assert_eq!(format_reset_at(200_000, 200_000 - 90 * 60), "in 1h 30m");
    assert!(!format_reset_at(200_000, 0).starts_with("in "));
}

#[test]
fn formats_provider_names_for_empty_window_notice() {
    assert_eq!(
        provider_names(&ProviderLimits {
            providers: vec![ProviderUsageLimits {
                provider: "xAI".into(),
                windows: vec![],
            }],
        }),
        "xAI"
    );
    assert_eq!(
        provider_names(&ProviderLimits {
            providers: vec![
                ProviderUsageLimits {
                    provider: "Codex".into(),
                    windows: vec![],
                },
                ProviderUsageLimits {
                    provider: "xAI".into(),
                    windows: vec![],
                },
            ],
        }),
        "Codex and xAI"
    );
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
fn present_limits_without_claude_cache_states_unknown_with_oauth_errors() {
    let view = present_limits_result(
        Ok((
            oauth_codex_limits(),
            vec![UsageLimitsError::Unauthorized {
                provider: "xAI",
                login: "/login xai-oauth",
            }],
        )),
        None,
        2_000,
    );
    let display = limits_display(&view);
    assert!(display
        .providers
        .iter()
        .all(|p| p.provider != "Claude Code"));
    assert!(view.items.iter().any(|item| matches!(
        item,
        LimitsViewItem::Error(text) if text.contains("xAI")
    )));
    assert_eq!(view.status, "OAuth usage limits partially updated");
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
        [
            LimitsViewItem::UsageLimits(limits),
            LimitsViewItem::Error(error)
        ] if limits.providers.is_empty() && error.contains("Codex")
    ));
    assert_eq!(view.status, "OAuth usage limit check failed");
}
