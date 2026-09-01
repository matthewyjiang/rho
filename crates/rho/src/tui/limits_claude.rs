//! Claude Code section of `/limits`: live PTY probe and overlay updates.
//!
//! Disk [`RateLimitState`] is the only Claude model. A successful probe writes
//! that cache; the overlay renders it. In-flight work is a pending fetch on
//! the shared `/limits` list.

use crate::claude_runtime::rate_limit::RateLimitState;
use crate::claude_runtime::usage_probe;
use crate::usage_limits::UsageLimitWindow;

use super::{
    empty_note, LimitsOverlay, LimitsSection, LimitsSectionId, LimitsSectionStatus,
    CLAUDE_CODE_PROVIDER_LABEL,
};

pub(super) struct ClaudeLimitsView<'a> {
    pub(super) disk: Option<&'a RateLimitState>,
    pub(super) pending: bool,
    pub(super) probe_available: bool,
}

pub(super) fn claude_probe_available() -> bool {
    if cfg!(test) {
        return false;
    }
    usage_probe::probe_supported() && crate::claude_runtime::executable::resolve().is_ok()
}

pub(super) fn claude_provider_limits(
    view: ClaudeLimitsView<'_>,
    now_unix: i64,
) -> Option<LimitsSection> {
    let disk_age = view.disk.and_then(RateLimitState::oldest_observed_unix);
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
            if include_age {
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

impl LimitsOverlay {
    fn claude_section_mut(&mut self) -> Option<&mut LimitsSection> {
        self.sections
            .iter_mut()
            .find(|section| section.id == LimitsSectionId::ClaudeCode)
    }

    pub(super) fn apply_claude_live(&mut self, windows: Vec<UsageLimitWindow>) {
        self.upsert_claude(windows, LimitsSectionStatus::Live);
    }

    pub(super) fn apply_claude_observed(&mut self, state: &RateLimitState, now_unix: i64) {
        let windows = claude_windows_from_state(state, now_unix, true);
        if windows.is_empty() {
            self.remove_claude();
            return;
        }
        self.upsert_claude(
            windows,
            LimitsSectionStatus::Observed {
                observed_at_unix: state.oldest_observed_unix().unwrap_or(now_unix),
            },
        );
    }

    pub(super) fn apply_claude_failed(&mut self, cached_at_unix: Option<i64>) {
        if let Some(section) = self.claude_section_mut() {
            section.status = LimitsSectionStatus::Failed { cached_at_unix };
            return;
        }
        self.sections.push(claude_section(
            LimitsSectionStatus::Failed { cached_at_unix },
            Vec::new(),
        ));
    }

    pub(super) fn set_claude_checking(&mut self, cached_at_unix: Option<i64>) {
        if let Some(section) = self.claude_section_mut() {
            section.status = LimitsSectionStatus::Checking { cached_at_unix };
            return;
        }
        self.sections.push(claude_section(
            LimitsSectionStatus::Checking { cached_at_unix },
            Vec::new(),
        ));
        self.empty_note = None;
    }

    pub(super) fn remove_claude(&mut self) {
        self.sections
            .retain(|section| section.id != LimitsSectionId::ClaudeCode);
        if self.sections.is_empty() {
            self.empty_note = Some(empty_note());
        }
    }

    fn upsert_claude(&mut self, windows: Vec<UsageLimitWindow>, status: LimitsSectionStatus) {
        let status = if windows.is_empty() {
            LimitsSectionStatus::Empty
        } else {
            status
        };
        if let Some(section) = self.claude_section_mut() {
            section.windows = windows;
            section.status = status;
            return;
        }
        self.sections.push(claude_section(status, windows));
        self.empty_note = None;
    }
}
