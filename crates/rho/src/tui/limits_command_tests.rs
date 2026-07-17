use pretty_assertions::assert_eq;

use super::*;

#[test]
fn running_limits_query_does_not_queue_model_context() {
    let mut app = super::super::tests::test_app();
    app.running = true;

    app.render_limits_result(Ok((
        ProviderLimits {
            providers: Vec::new(),
        },
        Vec::new(),
    )));

    assert!(app.steering_prompts.is_empty());
    assert!(app.queued_prompts.is_empty());
    assert!(matches!(
        app.transcript.last(),
        Some(Entry::Notice(notice))
            if notice == "no supported OAuth providers are connected; connect Codex with /login openai-codex, Kimi Code with /login kimi-code, or xAI with /login xai-oauth"
    ));
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
        Theme::limits_block(),
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
