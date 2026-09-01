use pretty_assertions::assert_eq;
use rho_providers::credentials::MemoryCredentialStore;
use std::{collections::BTreeMap, time::Instant};

use super::*;
use crate::usage_limits::{ProviderUsageLimits, UsageLimitWindow, UsageProviderKind};
use crate::usage_limits_cache::UsageLimitsCache;

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

fn codex_window() -> UsageLimitWindow {
    UsageLimitWindow {
        label: "Weekly".into(),
        remaining_percent: Some(40.0),
        resets_at_unix: Some(now_unix() + 3_600),
        note: None,
    }
}

// Covers: /limits must open a popup instead of inserting a transcript row or
// queuing a model turn.
// Owner: interactive TUI (unit seam; PTY covers the visible overlay)
#[test]
fn opening_limits_does_not_queue_model_context() {
    let mut app = super::super::tests::test_app();
    app.begin_provider_turn_ui();
    app.start_limits_command();

    assert!(app.pending.steering_prompts().is_empty());
    assert!(app.pending.queued_prompts().is_empty());
    assert!(matches!(
        app.input_ui.composer(),
        super::super::ComposerMode::Limits(_)
    ));
    assert!(
        app.history
            .entries()
            .iter()
            .all(|entry| !matches!(entry, super::super::Entry::Error(_))),
        "limits overlay must not dump a transcript error, got {:?}",
        app.history.entries()
    );
}

#[tokio::test]
async fn cancelling_limits_query_waits_for_background_task_to_stop() {
    let mut app = super::super::tests::test_app();
    let task_marker = std::sync::Arc::new(());
    let captured_marker = task_marker.clone();
    app.pending_usage_limits.push(PendingUsageFetch {
        id: LimitsSectionId::Provider(UsageProviderKind::Codex),
        handle: tokio::spawn(async move {
            let _marker = captured_marker;
            std::future::pending::<LimitsFetchResult>().await
        }),
    });

    app.cancel_limits_command().await;

    assert!(app.pending_usage_limits.is_empty());
    assert_eq!(std::sync::Arc::strong_count(&task_marker), 1);
}

#[test]
fn formats_reset_relative_only_within_one_day() {
    assert_eq!(format_reset_at(200_000, 200_000 - 90 * 60), "in 1h 30m");
    assert!(!format_reset_at(200_000, 0).starts_with("in "));
}

// Covers: a live fetch for one provider must not wait on the others.
// Owner: pure unit
#[test]
fn applying_one_provider_leaves_others_checking() {
    let mut overlay = LimitsOverlay {
        sections: vec![
            LimitsSection {
                id: LimitsSectionId::Provider(UsageProviderKind::Codex),
                label: UsageProviderKind::Codex.label().into(),
                status: LimitsSectionStatus::Checking {
                    cached_at_unix: None,
                },
                windows: Vec::new(),
            },
            LimitsSection {
                id: LimitsSectionId::Provider(UsageProviderKind::KimiCode),
                label: UsageProviderKind::KimiCode.label().into(),
                status: LimitsSectionStatus::Checking {
                    cached_at_unix: None,
                },
                windows: Vec::new(),
            },
        ],
        empty_note: None,
        scroll: Default::default(),
        checking_started: Instant::now(),
    };
    overlay.apply_live(
        LimitsSectionId::Provider(UsageProviderKind::Codex),
        UsageProviderKind::Codex.label(),
        vec![codex_window()],
    );

    assert_eq!(overlay.sections[0].status, LimitsSectionStatus::Live);
    assert_eq!(overlay.sections[0].windows.len(), 1);
    assert!(matches!(
        overlay.sections[1].status,
        LimitsSectionStatus::Checking {
            cached_at_unix: None
        }
    ));
    assert!(overlay.sections[1].windows.is_empty());
}

// Covers: reopening /limits after a live fetch must not invent an epoch-zero
// age when the on-disk cache is missing.
// Owner: pure unit
#[test]
fn live_ready_section_uses_in_memory_fetch_time_not_cache() {
    let windows = vec![codex_window()];
    let mut live = BTreeMap::new();
    live.insert(
        UsageProviderKind::Codex,
        LiveUsage::Ready {
            limits: ProviderUsageLimits {
                provider: UsageProviderKind::Codex.label().into(),
                windows: windows.clone(),
            },
            fetched_at_unix: 1_700,
        },
    );
    let section = provider_section(
        UsageProviderKind::Codex,
        &live,
        &[],
        &UsageLimitsCache::default(),
    );
    assert_eq!(section.status, LimitsSectionStatus::Live);
    assert_eq!(section.windows, windows);
}

// Covers: Claude observations appear without a live probe even with no OAuth.
// Owner: pure unit
#[test]
fn claude_section_is_present_without_oauth() {
    let overlay = build_limits_overlay(
        &MemoryCredentialStore::default(),
        &BTreeMap::new(),
        &[],
        UsageLimitsCache::default(),
        super::limits_claude::ClaudeLimitsView {
            disk: Some(&sample_claude_state(1_000)),
            pending: false,
            probe_available: false,
        },
        1_125,
    );
    assert_eq!(overlay.sections.len(), 1);
    assert_eq!(overlay.sections[0].id, LimitsSectionId::ClaudeCode);
    assert_eq!(overlay.sections[0].windows.len(), 1);
    assert_eq!(overlay.sections[0].windows[0].label, "5-hour");
    assert!(overlay.sections[0].windows[0].remaining_percent.is_none());
    assert!(matches!(
        overlay.sections[0].status,
        LimitsSectionStatus::Observed {
            observed_at_unix: 1_000
        }
    ));
}

#[test]
fn claude_section_surfaces_utilization_without_allowed_status() {
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

    let section = super::limits_claude::claude_provider_limits(
        super::limits_claude::ClaudeLimitsView {
            disk: Some(&state),
            pending: false,
            probe_available: false,
        },
        1_100,
    )
    .expect("claude");
    assert_eq!(section.windows[0].label, "5-hour");
    assert_eq!(section.windows[0].remaining_percent, Some(75.0));
    assert_eq!(section.windows[1].label, "Weekly");
    assert_eq!(section.windows[1].remaining_percent, Some(60.0));
}

// Covers: Live keeps cache-only windows but labels them with observed age.
// Owner: pure unit
#[test]
fn fresh_probe_state_shows_live_including_disk_fable() {
    let mut disk = sample_claude_state(1_100);
    disk.windows[0].info.utilization = Some(0.0);
    let mut weekly = disk.windows[0].clone();
    weekly.info.rate_limit_type = Some("seven_day".into());
    weekly.info.utilization = Some(0.19);
    weekly.observed_at_unix = 1_100;
    let mut fable = weekly.clone();
    fable.info.rate_limit_type = Some("seven_day_fable".into());
    fable.info.utilization = Some(0.33);
    fable.observed_at_unix = 1_000;
    fable.observed_at_nanos = 1_000_000_000_000;
    disk.merge_window(weekly);
    disk.merge_window(fable);
    disk.last_probe_unix = Some(1_100);

    let section = super::limits_claude::claude_provider_limits(
        super::limits_claude::ClaudeLimitsView {
            disk: Some(&disk),
            pending: false,
            probe_available: false,
        },
        1_100,
    )
    .expect("claude");
    assert_eq!(section.status, LimitsSectionStatus::Live);
    let labels: Vec<&str> = section
        .windows
        .iter()
        .map(|window| window.label.as_str())
        .collect();
    assert_eq!(labels, vec!["5-hour", "Weekly", "Fable"]);
    assert_eq!(section.windows[0].note, None);
    assert_eq!(section.windows[1].note, None);
    assert_eq!(section.windows[2].note.as_deref(), Some("observed 1m ago"));
}

// Covers: a live Claude probe must occupy /limits before any disk cache exists.
// Owner: pure unit
#[test]
fn stale_disk_probe_with_binary_shows_checking() {
    let mut disk = sample_claude_state(1_000);
    disk.last_probe_unix = Some(1);
    let overlay = build_limits_overlay(
        &MemoryCredentialStore::default(),
        &BTreeMap::new(),
        &[],
        UsageLimitsCache::default(),
        super::limits_claude::ClaudeLimitsView {
            disk: Some(&disk),
            pending: false,
            probe_available: true,
        },
        1_000 + 400,
    );
    assert!(matches!(
        overlay.sections[0].status,
        LimitsSectionStatus::Checking {
            cached_at_unix: Some(1)
        }
    ));
}

#[test]
fn claude_probe_without_disk_shows_checking() {
    let overlay = build_limits_overlay(
        &MemoryCredentialStore::default(),
        &BTreeMap::new(),
        &[],
        UsageLimitsCache::default(),
        super::limits_claude::ClaudeLimitsView {
            disk: None,
            pending: false,
            probe_available: true,
        },
        0,
    );
    assert_eq!(overlay.sections.len(), 1);
    assert_eq!(overlay.sections[0].id, LimitsSectionId::ClaudeCode);
    assert!(matches!(
        overlay.sections[0].status,
        LimitsSectionStatus::Checking {
            cached_at_unix: None
        }
    ));
}

#[test]
fn missing_claude_state_does_not_invent_a_section() {
    let overlay = build_limits_overlay(
        &MemoryCredentialStore::default(),
        &BTreeMap::new(),
        &[],
        UsageLimitsCache::default(),
        super::limits_claude::ClaudeLimitsView {
            disk: None,
            pending: false,
            probe_available: false,
        },
        0,
    );
    assert!(overlay.sections.is_empty());
    assert!(overlay.empty_note.is_some());
}

// Covers: bars share one label column across providers so they line up.
// Owner: pure unit
#[test]
fn overlay_body_uses_global_window_label_column() {
    let overlay = LimitsOverlay {
        sections: vec![
            LimitsSection {
                id: LimitsSectionId::Provider(UsageProviderKind::Codex),
                label: UsageProviderKind::Codex.label().into(),
                status: LimitsSectionStatus::Live,
                windows: vec![UsageLimitWindow {
                    label: "5-hour".into(),
                    remaining_percent: Some(40.0),
                    resets_at_unix: None,
                    note: None,
                }],
            },
            LimitsSection {
                id: LimitsSectionId::Provider(UsageProviderKind::Xai),
                label: UsageProviderKind::Xai.label().into(),
                status: LimitsSectionStatus::Live,
                windows: vec![UsageLimitWindow {
                    label: "Monthly".into(),
                    remaining_percent: Some(80.0),
                    resets_at_unix: None,
                    note: None,
                }],
            },
        ],
        empty_note: None,
        scroll: Default::default(),
        checking_started: Instant::now(),
    };
    let lines = overlay_body_lines(&overlay, 80, None, 10);
    let texts: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    let five = texts
        .iter()
        .find(|text| text.contains("5-hour"))
        .expect("5-hour row");
    let monthly = texts
        .iter()
        .find(|text| text.contains("Monthly"))
        .expect("Monthly row");
    let five_bar = five
        .find('█')
        .or_else(|| five.find('░'))
        .expect("codex bar");
    let monthly_bar = monthly
        .find('█')
        .or_else(|| monthly.find('░'))
        .expect("xai bar");
    assert_eq!(five_bar, monthly_bar);
}
