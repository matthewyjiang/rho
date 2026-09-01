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

/// Whether a row should carry an "observed Xs ago" note.
#[derive(Clone, Copy)]
enum AgeNote {
    Always,
    StaleOnly,
}

/// Overlay presentation derived from disk freshness, pending fetch, and probe.
#[derive(Clone, Copy)]
enum ClaudeDisplay {
    Checking { cached_at_unix: Option<i64> },
    Live,
    Observed { observed_at_unix: i64 },
}

impl ClaudeDisplay {
    fn age_note(self) -> AgeNote {
        match self {
            Self::Checking { .. } | Self::Observed { .. } => AgeNote::Always,
            Self::Live => AgeNote::StaleOnly,
        }
    }
}

pub(super) fn claude_probe_available() -> bool {
    // `probe_supported` is the Unix PTY capability and stays true under
    // `cfg(test)`. cargo test must not spawn the real `claude` TUI from
    // `/limits`; fake-child coverage goes through `read_usage_from_binary`.
    if cfg!(test) {
        return false;
    }
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
    let display = if view.pending || (view.probe_available && !fresh) {
        ClaudeDisplay::Checking {
            cached_at_unix: disk_age,
        }
    } else if fresh {
        ClaudeDisplay::Live
    } else {
        ClaudeDisplay::Observed {
            observed_at_unix: disk_age.unwrap_or(now_unix),
        }
    };
    let windows = view
        .disk
        .map(|state| claude_windows_from_state(state, now_unix, display.age_note()))
        .unwrap_or_default();
    match display {
        ClaudeDisplay::Checking { cached_at_unix } => Some(claude_section(
            LimitsSectionStatus::Checking { cached_at_unix },
            windows,
        )),
        ClaudeDisplay::Live => Some(claude_section(
            if windows.is_empty() {
                LimitsSectionStatus::Empty
            } else {
                LimitsSectionStatus::Live
            },
            windows,
        )),
        ClaudeDisplay::Observed { observed_at_unix } => {
            if windows.is_empty() {
                None
            } else {
                Some(claude_section(
                    LimitsSectionStatus::Observed { observed_at_unix },
                    windows,
                ))
            }
        }
    }
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
    pub(super) fn spawn_claude_usage_fetch(&mut self, disk: Option<&RateLimitState>) {
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
        if disk.is_some_and(|state| {
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
                            windows: claude_windows_from_state(
                                &state,
                                now_unix(),
                                AgeNote::StaleOnly,
                            ),
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
        let cached_at = disk.and_then(|state| state.section_age_unix());
        if let Some(overlay) = self.limits_overlay_mut() {
            set_claude_checking(overlay, cached_at);
        }
    }
}

pub(super) fn apply_claude_live(app: &mut App, windows: Vec<UsageLimitWindow>) {
    if let Some(overlay) = app.limits_overlay_mut() {
        overlay.apply_live(
            LimitsSectionId::ClaudeCode,
            CLAUDE_CODE_PROVIDER_LABEL,
            windows,
        );
    }
}

pub(super) fn apply_claude_disk_fallback(app: &mut App, failed: bool) {
    let Some(overlay) = app.limits_overlay_mut() else {
        return;
    };
    match crate::claude_runtime::rate_limit::load() {
        Some(state) if !state.is_empty() => {
            overlay.upsert(
                LimitsSectionId::ClaudeCode,
                CLAUDE_CODE_PROVIDER_LABEL,
                claude_windows_from_state(&state, now_unix(), AgeNote::Always),
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

fn claude_windows_from_state(
    state: &RateLimitState,
    now_unix: i64,
    age_note: AgeNote,
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
            let show_age = match age_note {
                AgeNote::Always => true,
                AgeNote::StaleOnly => cache_only,
            };
            if show_age {
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
