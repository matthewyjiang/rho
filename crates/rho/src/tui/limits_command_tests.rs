use pretty_assertions::assert_eq;

use super::*;

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
    // `render_limits_result` still reads the process Claude cache. Assert only
    // non-queueing and that presentation produced history; cache-dependent copy
    // is covered by `present_limits_*` tests with injected state.
    assert!(
        !app.history.entries().is_empty(),
        "{:?}",
        app.history.entries()
    );
    assert!(
        app.history.entries().iter().any(|entry| matches!(
            entry,
            Entry::Notice(notice) if notice.contains("claude code:")
        )),
        "expected a Claude limits notice, got {:?}",
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
        &ProviderLimits {
            providers: vec![ProviderUsageLimits {
                provider: "Codex".into(),
                windows: vec![UsageLimitWindow {
                    label: "Weekly".into(),
                    remaining_percent: 69.0,
                    resets_at_unix: now_unix() + 2 * 60 * 60 + 14 * 60,
                }],
            }],
        },
        80,
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(text[0].trim_end(), "OAuth usage limits");
    assert_eq!(text[2].trim_end(), "Codex");
    assert!(text[3].contains("Weekly"));
    assert!(text[3].contains("███████░░░"));
    assert!(text[3].contains("69% left"));
    assert!(text[3].contains("resets in 2h 14m"));
    assert!(!text.join("\n").contains("5-hour"));
    assert!(lines.iter().all(|line| line.width() == 80));
    assert!(lines.iter().all(|line| {
        line.style.bg.is_some() || line.spans.iter().all(|span| span.style.bg.is_some())
    }));
}

#[test]
fn renders_multiple_connected_providers() {
    let lines = usage_limit_lines(
        &ProviderLimits {
            providers: vec![
                ProviderUsageLimits {
                    provider: "Codex".into(),
                    windows: vec![UsageLimitWindow {
                        label: "Weekly".into(),
                        remaining_percent: 69.0,
                        resets_at_unix: now_unix() + 2 * 60 * 60 + 14 * 60,
                    }],
                },
                ProviderUsageLimits {
                    provider: "xAI".into(),
                    windows: vec![UsageLimitWindow {
                        label: "Weekly".into(),
                        remaining_percent: 97.0,
                        resets_at_unix: now_unix() + 3 * 24 * 60 * 60,
                    }],
                },
            ],
        },
        80,
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(text[2].trim_end(), "Codex");
    assert!(text.iter().any(|line| line.trim_end() == "xAI"));
    assert!(text.iter().any(|line| line.contains("97% left")));
}

#[test]
fn narrow_layout_wraps_reset_instead_of_hiding_it() {
    let lines = usage_limit_window_lines(
        &UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: 93.0,
            resets_at_unix: 10_000,
        },
        6,
        43,
        10_000 - 2 * 60 * 60 - 14 * 60,
        Theme::command_block(),
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

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
    let window = UsageLimitWindow {
        label: "Weekly".into(),
        remaining_percent: 50.0,
        resets_at_unix: 200_000,
    };
    assert_eq!(format_reset(&window, 200_000 - 90 * 60), "in 1h 30m");
    assert!(!format_reset(&window, 0).starts_with("in "));
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

fn sample_claude(observed_at_unix: i64) -> crate::claude_runtime::rate_limit::ObservedRateLimit {
    crate::claude_runtime::rate_limit::ObservedRateLimit {
        observed_at_unix,
        observed_seq: 1,
        info: crate::claude_runtime::stream::RateLimitInfo {
            status: Some("allowed".into()),
            rate_limit_type: Some("five_hour".into()),
            resets_at: Some(1_800_000_000),
            overage_status: None,
            overage_resets_at: None,
            is_using_overage: Some(false),
        },
    }
}

fn oauth_codex_limits() -> ProviderLimits {
    ProviderLimits {
        providers: vec![ProviderUsageLimits {
            provider: "Codex".into(),
            windows: vec![UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: 40.0,
                resets_at_unix: now_unix() + 3_600,
            }],
        }],
    }
}

fn notice_texts(view: &LimitsView) -> Vec<&str> {
    view.items
        .iter()
        .filter_map(|item| match item {
            LimitsViewItem::Notice(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn present_limits_without_claude_cache_states_unknown_even_with_oauth_data() {
    let view = present_limits_result(Ok((oauth_codex_limits(), Vec::new())), None, 2_000);
    let notices = notice_texts(&view);
    assert!(
        notices
            .iter()
            .any(|text| text.contains("no limit observation is known yet")),
        "{notices:?}"
    );
    assert!(view
        .items
        .iter()
        .any(|item| matches!(item, LimitsViewItem::UsageLimits(_))));
    assert!(!notices.iter().any(|text| text.contains('%')));
    assert_eq!(view.status, "OAuth usage limits updated");
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
    let notices = notice_texts(&view);
    assert!(
        notices
            .iter()
            .any(|text| text.contains("no limit observation is known yet")),
        "{notices:?}"
    );
    assert!(view.items.iter().any(|item| matches!(
        item,
        LimitsViewItem::Error(text) if text.contains("xAI")
    )));
    assert_eq!(view.status, "OAuth usage limits partially updated");
}

#[test]
fn present_limits_with_claude_cache_shows_age_without_percentage() {
    let observed = sample_claude(1_000);
    let view = present_limits_result(
        Ok((oauth_codex_limits(), Vec::new())),
        Some(&observed),
        1_000 + 125,
    );
    let notices = notice_texts(&view);
    let claude = notices
        .iter()
        .find(|text| text.contains("claude code:"))
        .copied()
        .expect("claude notice");
    assert!(claude.contains("five hour"), "{claude}");
    assert!(claude.contains("allowed"), "{claude}");
    assert!(claude.contains("last observed 2m ago"), "{claude}");
    assert!(!claude.contains('%'), "{claude}");
    assert!(!claude.contains("known yet"), "{claude}");
}

#[test]
fn present_limits_claude_only_when_no_oauth_providers() {
    let observed = sample_claude(500);
    let view = present_limits_result(
        Ok((
            ProviderLimits {
                providers: Vec::new(),
            },
            Vec::new(),
        )),
        Some(&observed),
        560,
    );
    assert_eq!(view.items.len(), 1);
    assert!(matches!(
        &view.items[0],
        LimitsViewItem::Notice(text)
            if text.contains("claude code:") && text.contains("last observed 1m ago")
    ));
    assert_eq!(view.status, "claude code limits only");
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
            LimitsViewItem::Notice(notice),
            LimitsViewItem::Error(error)
        ] if notice.contains("no limit observation is known yet")
            && error.contains("Codex")
    ));
    assert_eq!(view.status, "OAuth usage limit check failed");
}
