//! Claude Code section of `/limits`: live PTY probe and overlay updates.
//!
//! Disk [`RateLimitState`] is the only Claude model. A successful probe writes
//! that cache; the overlay renders it. In-flight work is a pending fetch on
//! the shared `/limits` list.

use crate::claude_runtime::rate_limit::RateLimitState;
use crate::claude_runtime::usage_probe;
use crate::usage_limits::UsageLimitWindow;

use super::{
    now_unix, App, LimitsFetchResult, LimitsOverlay, LimitsSection, LimitsSectionId,
    LimitsSectionStatus, PendingUsageFetch, CLAUDE_CODE_PROVIDER_LABEL,
};

pub(super) struct ClaudeLimitsView<'a> {
    pub(super) disk: Option<&'a RateLimitState>,
    pub(super) pending: bool,
    pub(super) probe_available: bool,
}

pub(super) fn claude_probe_available() -> bool {
    usage_probe::probe_supported() && crate::claude_runtime::executable::resolve().is_ok()
}

pub(super) fn claude_provider_limits(
    view: ClaudeLimitsView<'_>,
    now_unix: i64,
) -> Option<LimitsSection> {
    let disk_age = view.disk.and_then(RateLimitState::section_age_unix);
    let fresh = view.disk.is_some_and(|state| {
        state
            .last_probe_unix
            .is_some_and(|at| usage_probe::live_is_fresh(at, now_unix))
    });
    let disk_windows = view
        .disk
        .map(|state| claude_windows_from_state(state, now_unix, !fresh || view.pending))
        .unwrap_or_default();

    if view.pending || (view.probe_available && !fresh) {
        return Some(claude_section(
            LimitsSectionStatus::Checking {
                cached_at_unix: disk_age,
            },
            disk_windows,
        ));
    }
    if fresh {
        let windows = view
            .disk
            .map(|state| claude_windows_from_state(state, now_unix, false))
            .unwrap_or_default();
        return Some(claude_section(
            if windows.is_empty() {
                LimitsSectionStatus::Empty
            } else {
                LimitsSectionStatus::Live
            },
            windows,
        ));
    }
    if disk_windows.is_empty() {
        return None;
    }
    Some(claude_section(
        LimitsSectionStatus::Observed {
            observed_at_unix: disk_age.unwrap_or(now_unix),
        },
        disk_windows,
    ))
}

fn claude_section(status: LimitsSectionStatus, windows: Vec<UsageLimitWindow>) -> LimitsSection {
    LimitsSection {
        id: LimitsSectionId::ClaudeCode,
        label: CLAUDE_CODE_PROVIDER_LABEL.into(),
        status,
        windows,
    }
}

impl App {
    pub(super) fn spawn_claude_usage_fetch(&mut self) {
        if self
            .pending_usage_limits
            .iter()
            .any(|fetch| fetch.id == LimitsSectionId::ClaudeCode)
        {
            return;
        }
        if !claude_probe_available() {
            return;
        }
        let now = now_unix();
        if crate::claude_runtime::rate_limit::load().is_some_and(|state| {
            state
                .last_probe_unix
                .is_some_and(|at| usage_probe::live_is_fresh(at, now))
        }) {
            return;
        }
        self.pending_usage_limits.push(PendingUsageFetch {
            id: LimitsSectionId::ClaudeCode,
            handle: tokio::spawn(async {
                match usage_probe::fetch_usage().await {
                    Ok(usage_probe::UsageProbeOutcome::Ready(state)) => {
                        LimitsFetchResult::ClaudeReady {
                            windows: claude_windows_from_state(&state, now_unix(), false),
                        }
                    }
                    Ok(usage_probe::UsageProbeOutcome::Unavailable) => {
                        LimitsFetchResult::Unavailable
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "claude usage probe failed");
                        LimitsFetchResult::Failed
                    }
                }
            }),
        });
        let cached_at =
            crate::claude_runtime::rate_limit::load().and_then(|state| state.section_age_unix());
        if let Some(overlay) = self.limits_overlay_mut() {
            set_claude_checking(overlay, cached_at);
        }
    }
}

pub(super) fn apply_claude_limits_result(app: &mut App, result: LimitsFetchResult) {
    match result {
        LimitsFetchResult::ClaudeReady { windows } => {
            if let Some(overlay) = app.limits_overlay_mut() {
                overlay.apply_live(
                    LimitsSectionId::ClaudeCode,
                    CLAUDE_CODE_PROVIDER_LABEL,
                    windows,
                );
            }
        }
        LimitsFetchResult::ProviderReady { .. } => apply_claude_disk_fallback(app, true),
        LimitsFetchResult::Unavailable => apply_claude_disk_fallback(app, false),
        LimitsFetchResult::Failed => apply_claude_disk_fallback(app, true),
    }
}

fn apply_claude_disk_fallback(app: &mut App, failed: bool) {
    let Some(overlay) = app.limits_overlay_mut() else {
        return;
    };
    match crate::claude_runtime::rate_limit::load() {
        Some(state) if !state.is_empty() => {
            overlay.upsert(
                LimitsSectionId::ClaudeCode,
                CLAUDE_CODE_PROVIDER_LABEL,
                claude_windows_from_state(&state, now_unix(), true),
                LimitsSectionStatus::Observed {
                    observed_at_unix: state.section_age_unix().unwrap_or(now_unix()),
                },
            );
        }
        state if failed => {
            overlay.apply_failed(
                LimitsSectionId::ClaudeCode,
                state.and_then(|value| value.section_age_unix()),
                Some(CLAUDE_CODE_PROVIDER_LABEL),
            );
        }
        _ => overlay.remove_id(LimitsSectionId::ClaudeCode),
    }
}

fn set_claude_checking(overlay: &mut LimitsOverlay, cached_at_unix: Option<i64>) {
    if let Some(section) = overlay.section_mut(LimitsSectionId::ClaudeCode) {
        section.status = LimitsSectionStatus::Checking { cached_at_unix };
        return;
    }
    overlay.upsert(
        LimitsSectionId::ClaudeCode,
        CLAUDE_CODE_PROVIDER_LABEL,
        Vec::new(),
        LimitsSectionStatus::Checking { cached_at_unix },
    );
}

pub(super) fn claude_windows_from_state(
    state: &RateLimitState,
    now_unix: i64,
    include_age: bool,
) -> Vec<UsageLimitWindow> {
    state
        .sorted_windows()
        .into_iter()
        .map(|window| {
            let mut note_parts = Vec::new();
            if let Some(status) = crate::claude_runtime::stream::notable_rate_limit_status(
                window.info.status.as_deref(),
            ) {
                note_parts.push(status);
            }
            if window.info.is_using_overage == Some(true) {
                note_parts.push("using overage".into());
            }
            let cache_only = match state.last_probe_unix {
                Some(probe) => window.observed_at_unix < probe,
                None => true,
            };
            if include_age || cache_only {
                if let Some(age) = crate::claude_runtime::rate_limit::format_age_since(
                    window.observed_at_unix,
                    now_unix,
                ) {
                    note_parts.push(format!("observed {age}"));
                }
            }
            UsageLimitWindow {
                label: window.info.window_label(),
                remaining_percent: window.info.remaining_percent(),
                resets_at_unix: window.info.resets_at,
                note: (!note_parts.is_empty()).then(|| note_parts.join(", ")),
            }
        })
        .collect()
}
