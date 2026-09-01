use std::{collections::BTreeMap, time::Instant};

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use rho_providers::credentials::CredentialStore;

use super::{
    activity::LoadingSpinner,
    overlay_panel::{
        clamp_panel_scroll, overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel,
        OverlayPanelFrame,
    },
    render::{display_width, wrap_line_at_whitespace},
    theme::Theme,
    App, ComposerMode,
};
use crate::usage_limits::{
    fetch_usage_provider, now_unix, usage_provider_is_connected, UsageLimitWindow,
    UsageProviderKind,
};
use crate::usage_limits_cache::{self, UsageLimitsCache};

#[path = "limits_claude.rs"]
mod limits_claude;

const BAR_WIDTH: usize = 10;
const RELATIVE_RESET_CUTOFF_SECONDS: i64 = 24 * 60 * 60;
const TITLE: &str = "Usage limits";
const FOOTER: &str = "Enter/Esc close";

/// Display label for Claude Code limits. Presentation only — identity uses
/// [`LimitsSectionId::ClaudeCode`].
const CLAUDE_CODE_PROVIDER_LABEL: &str = "Claude Code";

enum LimitsFetchResult {
    ProviderReady {
        limits: crate::usage_limits::ProviderUsageLimits,
    },
    ClaudeReady {
        windows: Vec<UsageLimitWindow>,
    },
    Unavailable,
    Failed,
}

pub(super) struct PendingUsageFetch {
    id: LimitsSectionId,
    handle: tokio::task::JoinHandle<LimitsFetchResult>,
}

impl PendingUsageFetch {
    pub(super) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    fn provider_kind(&self) -> Option<UsageProviderKind> {
        self.id.provider_kind()
    }
}

#[derive(Clone, Debug)]
pub(super) enum LiveUsage {
    Ready {
        limits: crate::usage_limits::ProviderUsageLimits,
        fetched_at_unix: i64,
    },
    Failed,
}

#[derive(Clone, Copy, Debug)]
enum LimitsScrollTarget {
    Delta(isize),
    Page(isize),
    Absolute(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LimitsSectionId {
    Provider(UsageProviderKind),
    ClaudeCode,
}

impl LimitsSectionId {
    fn provider_kind(self) -> Option<UsageProviderKind> {
        match self {
            Self::Provider(kind) => Some(kind),
            Self::ClaudeCode => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LimitsSectionStatus {
    Checking { cached_at_unix: Option<i64> },
    Live,
    Observed { observed_at_unix: i64 },
    Failed { cached_at_unix: Option<i64> },
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
struct LimitsSection {
    id: LimitsSectionId,
    label: String,
    status: LimitsSectionStatus,
    windows: Vec<UsageLimitWindow>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LimitsOverlay {
    sections: Vec<LimitsSection>,
    empty_note: Option<String>,
    scroll: usize,
    checking_started: Instant,
}

impl LimitsOverlay {
    pub(super) fn is_checking(&self) -> bool {
        self.sections
            .iter()
            .any(|section| matches!(section.status, LimitsSectionStatus::Checking { .. }))
    }

    fn apply_live(&mut self, id: LimitsSectionId, label: &str, windows: Vec<UsageLimitWindow>) {
        self.upsert(id, label, windows, LimitsSectionStatus::Live);
    }

    fn apply_failed(
        &mut self,
        id: LimitsSectionId,
        cached_at_unix: Option<i64>,
        insert_label: Option<&str>,
    ) {
        let status = LimitsSectionStatus::Failed { cached_at_unix };
        if let Some(section) = self.section_mut(id) {
            section.status = status;
            return;
        }
        if let Some(label) = insert_label {
            self.sections.push(LimitsSection {
                id,
                label: label.into(),
                status,
                windows: Vec::new(),
            });
            self.empty_note = None;
        }
    }

    fn section_mut(&mut self, id: LimitsSectionId) -> Option<&mut LimitsSection> {
        self.sections.iter_mut().find(|section| section.id == id)
    }

    fn remove_id(&mut self, id: LimitsSectionId) {
        self.sections.retain(|section| section.id != id);
        if self.sections.is_empty() {
            self.empty_note = Some(empty_note());
        }
    }

    fn upsert(
        &mut self,
        id: LimitsSectionId,
        label: &str,
        windows: Vec<UsageLimitWindow>,
        status: LimitsSectionStatus,
    ) {
        let status = if windows.is_empty() && matches!(status, LimitsSectionStatus::Live) {
            LimitsSectionStatus::Empty
        } else {
            status
        };
        if let Some(section) = self.section_mut(id) {
            section.windows = windows;
            section.status = status;
            return;
        }
        self.sections.push(LimitsSection {
            id,
            label: label.into(),
            status,
            windows,
        });
        self.empty_note = None;
    }

    fn scroll_by(&mut self, delta: isize, body_len: usize, body_rows: usize) {
        let next = if delta < 0 {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize)
        };
        self.scroll = clamp_panel_scroll(next, body_len, body_rows);
    }
}

impl App {
    pub(super) fn execute_limits_command(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.start_limits_command();
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    pub(super) fn start_limits_command(&mut self) {
        let claude_disk = crate::claude_runtime::rate_limit::load();
        self.open_limits_overlay(claude_disk.as_ref());
        self.spawn_missing_usage_fetches(claude_disk.as_ref());
        self.set_status("usage limits");
    }

    pub(super) fn limits_overlay_open(&self) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Limits(_))
    }

    pub(super) fn close_limits_overlay(&mut self) {
        if self.limits_overlay_open() {
            self.input_ui.set_composer(ComposerMode::Input);
        }
    }

    pub(super) async fn cancel_limits_command(&mut self) {
        let pending = std::mem::take(&mut self.pending_usage_limits);
        for fetch in pending {
            fetch.handle.abort();
            let _ = fetch.handle.await;
        }
    }

    pub(super) async fn poll_limits_command(&mut self) -> anyhow::Result<bool> {
        let mut changed = false;
        let mut still_pending = Vec::new();
        let pending = std::mem::take(&mut self.pending_usage_limits);
        for fetch in pending {
            if !fetch.is_finished() {
                still_pending.push(fetch);
                continue;
            }
            changed = true;
            match fetch.handle.await {
                Ok(result) => self.apply_limits_fetch(fetch.id, result),
                Err(_) => self.apply_limits_fetch(fetch.id, LimitsFetchResult::Failed),
            }
        }
        self.pending_usage_limits = still_pending;
        Ok(changed)
    }

    pub(super) fn limits_overlay_frame(
        &self,
        area: Rect,
        now: Instant,
    ) -> Option<OverlayPanelFrame> {
        let ComposerMode::Limits(overlay) = self.input_ui.composer() else {
            return None;
        };
        let spinner = overlay
            .is_checking()
            .then(|| LoadingSpinner::frame_since(overlay.checking_started, now));
        let inner_width = overlay_panel_inner_width(area);
        let body = overlay_body_lines(overlay, inner_width, spinner, now_unix());
        Some(render_overlay_panel(
            TITLE,
            FOOTER,
            &body,
            overlay.scroll,
            area,
        ))
    }

    pub(super) fn handle_limits_overlay_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &ratatui::DefaultTerminal,
    ) -> bool {
        if !self.limits_overlay_open() {
            return false;
        }
        match (key.modifiers, key.code) {
            (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Esc)
            | (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Enter)
            | (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Char('q')) => {
                self.close_limits_overlay();
                true
            }
            (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Up)
            | (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Char('k')) => {
                self.scroll_limits_overlay(terminal, -1);
                true
            }
            (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Down)
            | (crossterm::event::KeyModifiers::NONE, crossterm::event::KeyCode::Char('j')) => {
                self.scroll_limits_overlay(terminal, 1);
                true
            }
            (_, crossterm::event::KeyCode::PageUp) => {
                self.scroll_limits_overlay_page(terminal, -1);
                true
            }
            (_, crossterm::event::KeyCode::PageDown) => {
                self.scroll_limits_overlay_page(terminal, 1);
                true
            }
            (_, crossterm::event::KeyCode::Home) => {
                self.set_limits_overlay_scroll(terminal, 0);
                true
            }
            (_, crossterm::event::KeyCode::End) => {
                self.set_limits_overlay_scroll(terminal, usize::MAX);
                true
            }
            (crossterm::event::KeyModifiers::CONTROL, crossterm::event::KeyCode::Char('c')) => {
                false
            }
            _ => true,
        }
    }

    pub(super) fn scroll_limits_overlay_wheel(
        &mut self,
        width: u16,
        height: u16,
        delta: isize,
    ) -> bool {
        if !self.limits_overlay_open() {
            return false;
        }
        self.scroll_limits_overlay_area(Rect::new(0, 0, width, height), delta);
        true
    }

    fn scroll_limits_overlay(&mut self, terminal: &ratatui::DefaultTerminal, delta: isize) {
        self.apply_limits_scroll(terminal, LimitsScrollTarget::Delta(delta));
    }

    fn scroll_limits_overlay_page(
        &mut self,
        terminal: &ratatui::DefaultTerminal,
        direction: isize,
    ) {
        self.apply_limits_scroll(terminal, LimitsScrollTarget::Page(direction));
    }

    fn set_limits_overlay_scroll(&mut self, terminal: &ratatui::DefaultTerminal, scroll: usize) {
        self.apply_limits_scroll(terminal, LimitsScrollTarget::Absolute(scroll));
    }

    fn apply_limits_scroll(
        &mut self,
        terminal: &ratatui::DefaultTerminal,
        target: LimitsScrollTarget,
    ) {
        let Ok(size) = terminal.size() else {
            return;
        };
        self.apply_limits_scroll_area(Rect::new(0, 0, size.width, size.height), target);
    }

    fn scroll_limits_overlay_area(&mut self, area: Rect, delta: isize) {
        self.apply_limits_scroll_area(area, LimitsScrollTarget::Delta(delta));
    }

    fn apply_limits_scroll_area(&mut self, area: Rect, target: LimitsScrollTarget) {
        let Some((body_len, body_rows)) = self.limits_scroll_metrics(area) else {
            return;
        };
        let ComposerMode::Limits(overlay) = self.input_ui.composer_mut() else {
            return;
        };
        match target {
            LimitsScrollTarget::Delta(delta) => overlay.scroll_by(delta, body_len, body_rows),
            LimitsScrollTarget::Page(direction) => overlay.scroll_by(
                direction.saturating_mul(body_rows.max(1) as isize),
                body_len,
                body_rows,
            ),
            LimitsScrollTarget::Absolute(scroll) => {
                overlay.scroll = clamp_panel_scroll(scroll, body_len, body_rows);
            }
        }
    }

    fn limits_scroll_metrics(&self, area: Rect) -> Option<(usize, usize)> {
        let ComposerMode::Limits(overlay) = self.input_ui.composer() else {
            return None;
        };
        let inner_width = overlay_panel_inner_width(area);
        let body_len = overlay_body_lines(overlay, inner_width, None, now_unix()).len();
        let body_rows = overlay_panel_layout(area, body_len).body_rows;
        Some((body_len, body_rows))
    }

    fn limits_overlay_mut(&mut self) -> Option<&mut LimitsOverlay> {
        match self.input_ui.composer_mut() {
            ComposerMode::Limits(overlay) => Some(overlay),
            _ => None,
        }
    }

    fn open_limits_overlay(
        &mut self,
        claude_disk: Option<&crate::claude_runtime::rate_limit::RateLimitState>,
    ) {
        let overlay = build_limits_overlay(
            self.credential_store.as_ref(),
            &self.usage_limits_live,
            self.pending_usage_limits
                .iter()
                .filter_map(PendingUsageFetch::provider_kind)
                .collect::<Vec<_>>()
                .as_slice(),
            usage_limits_cache::load(),
            limits_claude::ClaudeLimitsView {
                disk: claude_disk,
                pending: self
                    .pending_usage_limits
                    .iter()
                    .any(|fetch| fetch.id == LimitsSectionId::ClaudeCode),
                probe_available: limits_claude::claude_probe_available(),
            },
            crate::claude_runtime::rate_limit::now_unix(),
        );
        self.input_ui.set_composer(ComposerMode::Limits(overlay));
    }

    fn spawn_missing_usage_fetches(
        &mut self,
        claude_disk: Option<&crate::claude_runtime::rate_limit::RateLimitState>,
    ) {
        let pending: Vec<UsageProviderKind> = self
            .pending_usage_limits
            .iter()
            .filter_map(PendingUsageFetch::provider_kind)
            .collect();
        let kinds = connected_kinds(self.credential_store.as_ref());
        for kind in kinds {
            if pending.contains(&kind) {
                continue;
            }
            let store = self.credential_store.clone();
            let client = self
                .usage_limits_client
                .get_or_init(crate::reqwest_client)
                .clone();
            self.pending_usage_limits.push(PendingUsageFetch {
                id: LimitsSectionId::Provider(kind),
                handle: tokio::spawn(async move {
                    match fetch_usage_provider(kind, store.as_ref(), client).await {
                        Ok(Some(limits)) => LimitsFetchResult::ProviderReady { limits },
                        Ok(None) => LimitsFetchResult::Unavailable,
                        Err(_) => LimitsFetchResult::Failed,
                    }
                }),
            });
            let live_fetched_at = match self.usage_limits_live.get(&kind) {
                Some(LiveUsage::Ready {
                    fetched_at_unix, ..
                }) => Some(*fetched_at_unix),
                _ => None,
            };
            if let Some(overlay) = self.limits_overlay_mut() {
                if let Some(section) = overlay.section_mut(LimitsSectionId::Provider(kind)) {
                    let cached_at_unix = match section.status {
                        LimitsSectionStatus::Live => live_fetched_at,
                        LimitsSectionStatus::Checking { cached_at_unix }
                        | LimitsSectionStatus::Failed { cached_at_unix } => cached_at_unix,
                        LimitsSectionStatus::Observed { .. } | LimitsSectionStatus::Empty => None,
                    };
                    section.status = LimitsSectionStatus::Checking { cached_at_unix };
                }
            }
        }
        self.spawn_claude_usage_fetch(claude_disk);
    }

    fn apply_limits_fetch(&mut self, id: LimitsSectionId, result: LimitsFetchResult) {
        match result {
            LimitsFetchResult::ProviderReady { limits } => {
                if let Some(kind) = id.provider_kind() {
                    self.apply_provider_ready(kind, limits);
                }
            }
            LimitsFetchResult::ClaudeReady { windows } => {
                if id == LimitsSectionId::ClaudeCode {
                    limits_claude::apply_claude_live(self, windows);
                }
            }
            LimitsFetchResult::Unavailable => match id {
                LimitsSectionId::Provider(kind) => {
                    self.usage_limits_live.remove(&kind);
                    if let Some(overlay) = self.limits_overlay_mut() {
                        overlay.remove_id(LimitsSectionId::Provider(kind));
                    }
                }
                LimitsSectionId::ClaudeCode => {
                    limits_claude::apply_claude_disk_fallback(self, false)
                }
            },
            LimitsFetchResult::Failed => match id {
                LimitsSectionId::Provider(kind) => self.mark_usage_failed(kind),
                LimitsSectionId::ClaudeCode => {
                    limits_claude::apply_claude_disk_fallback(self, true)
                }
            },
        }
    }

    fn apply_provider_ready(
        &mut self,
        kind: UsageProviderKind,
        limits: crate::usage_limits::ProviderUsageLimits,
    ) {
        let fetched_at_unix = now_unix();
        let windows = limits.windows.clone();
        let mut cache = usage_limits_cache::load();
        cache.upsert(kind, limits.windows.clone(), fetched_at_unix);
        let _ = usage_limits_cache::save(&cache);
        self.usage_limits_live.insert(
            kind,
            LiveUsage::Ready {
                limits,
                fetched_at_unix,
            },
        );
        if let Some(overlay) = self.limits_overlay_mut() {
            overlay.apply_live(LimitsSectionId::Provider(kind), kind.label(), windows);
        }
    }

    pub(super) fn clamp_limits_overlay_scroll(&mut self, terminal: &ratatui::DefaultTerminal) {
        if !self.limits_overlay_open() {
            return;
        }
        let ComposerMode::Limits(overlay) = self.input_ui.composer() else {
            return;
        };
        let scroll = overlay.scroll;
        self.set_limits_overlay_scroll(terminal, scroll);
    }

    fn mark_usage_failed(&mut self, kind: UsageProviderKind) {
        self.usage_limits_live.insert(kind, LiveUsage::Failed);
        let cached_at = usage_limits_cache::load()
            .get(kind)
            .map(|entry| entry.fetched_at_unix);
        if let Some(overlay) = self.limits_overlay_mut() {
            overlay.apply_failed(LimitsSectionId::Provider(kind), cached_at, None);
        }
    }
}

fn connected_kinds(store: &dyn CredentialStore) -> Vec<UsageProviderKind> {
    UsageProviderKind::ALL
        .into_iter()
        .filter(|kind| usage_provider_is_connected(*kind, store).unwrap_or(false))
        .collect()
}

fn empty_note() -> String {
    "no supported providers are connected and no Claude Code limits are known yet; connect Codex with /login openai-codex, Kimi Code with /login kimi-code, xAI with /login xai-oauth, OpenCode Go with /login opencode-go, or sign in with /login claude-code"
        .into()
}

fn build_limits_overlay(
    store: &dyn CredentialStore,
    live: &BTreeMap<UsageProviderKind, LiveUsage>,
    pending: &[UsageProviderKind],
    cache: UsageLimitsCache,
    claude: limits_claude::ClaudeLimitsView<'_>,
    now_unix: i64,
) -> LimitsOverlay {
    let mut sections = Vec::new();
    for kind in UsageProviderKind::ALL {
        match usage_provider_is_connected(kind, store) {
            Ok(false) => {}
            Ok(true) => sections.push(provider_section(kind, live, pending, &cache)),
            Err(_) => {
                let cached = cache.get(kind);
                sections.push(LimitsSection {
                    id: LimitsSectionId::Provider(kind),
                    label: kind.label().into(),
                    status: LimitsSectionStatus::Failed {
                        cached_at_unix: cached.map(|entry| entry.fetched_at_unix),
                    },
                    windows: cached
                        .map(|entry| entry.windows.clone())
                        .unwrap_or_default(),
                });
            }
        }
    }
    if let Some(claude) = limits_claude::claude_provider_limits(claude, now_unix) {
        sections.push(claude);
    }
    let empty_note = sections.is_empty().then(empty_note);
    LimitsOverlay {
        sections,
        empty_note,
        scroll: 0,
        checking_started: Instant::now(),
    }
}

fn provider_section(
    kind: UsageProviderKind,
    live: &BTreeMap<UsageProviderKind, LiveUsage>,
    pending: &[UsageProviderKind],
    cache: &UsageLimitsCache,
) -> LimitsSection {
    let cached = cache.get(kind);
    let checking = pending.contains(&kind);
    match live.get(&kind) {
        Some(LiveUsage::Ready { limits, .. }) if !checking => LimitsSection {
            id: LimitsSectionId::Provider(kind),
            label: kind.label().into(),
            status: if limits.windows.is_empty() {
                LimitsSectionStatus::Empty
            } else {
                LimitsSectionStatus::Live
            },
            windows: limits.windows.clone(),
        },
        Some(LiveUsage::Failed) if !checking => LimitsSection {
            id: LimitsSectionId::Provider(kind),
            label: kind.label().into(),
            status: LimitsSectionStatus::Failed {
                cached_at_unix: cached.map(|entry| entry.fetched_at_unix),
            },
            windows: cached
                .map(|entry| entry.windows.clone())
                .unwrap_or_default(),
        },
        _ => LimitsSection {
            id: LimitsSectionId::Provider(kind),
            label: kind.label().into(),
            status: LimitsSectionStatus::Checking {
                cached_at_unix: cached.map(|entry| entry.fetched_at_unix),
            },
            windows: cached
                .map(|entry| entry.windows.clone())
                .unwrap_or_default(),
        },
    }
}

fn overlay_body_lines(
    overlay: &LimitsOverlay,
    width: usize,
    spinner: Option<&str>,
    now_unix: i64,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(note) = &overlay.empty_note {
        lines.extend(indented_note_lines(note, width, Theme::dim()));
        return lines;
    }

    let label_width = overlay
        .sections
        .iter()
        .flat_map(|section| section.windows.iter())
        .map(|window| display_width(&window.label))
        .max()
        .unwrap_or(0)
        .max(display_width("Usage"));

    for (index, section) in overlay.sections.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.push(section_heading(section, width, spinner, now_unix));
        if section.windows.is_empty() {
            match section.status {
                LimitsSectionStatus::Checking { .. } => {
                    lines.push(placeholder_window_line(label_width, width));
                }
                LimitsSectionStatus::Empty
                | LimitsSectionStatus::Failed { .. }
                | LimitsSectionStatus::Live
                | LimitsSectionStatus::Observed { .. } => {
                    lines.push(Line::from(Span::styled(
                        "  no active usage limit windows reported",
                        Theme::dim(),
                    )));
                }
            }
            continue;
        }
        lines.extend(section.windows.iter().flat_map(|window| {
            usage_limit_window_lines(window, label_width, width, now_unix, Theme::text())
        }));
    }
    lines
}

fn section_heading(
    section: &LimitsSection,
    width: usize,
    spinner: Option<&str>,
    now_unix: i64,
) -> Line<'static> {
    let status = heading_status(section, spinner, now_unix);
    let label = section.label.clone();
    let label_style = Theme::text().add_modifier(Modifier::BOLD);
    if status.is_empty() || width == 0 {
        return Line::from(Span::styled(truncate_to(label, width), label_style));
    }
    let gap = 2;
    let status_width = display_width(&status);
    let label_budget = width.saturating_sub(status_width.saturating_add(gap));
    let label = truncate_to(label, label_budget);
    let pad = width
        .saturating_sub(display_width(&label))
        .saturating_sub(status_width);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(status, Theme::dim()),
    ])
}

fn heading_status(section: &LimitsSection, spinner: Option<&str>, now_unix: i64) -> String {
    match section.status {
        LimitsSectionStatus::Checking { cached_at_unix } => {
            let spin = spinner.unwrap_or("⠙");
            match cached_at_unix.and_then(|cached_at| {
                crate::claude_runtime::rate_limit::format_age_since(cached_at, now_unix)
            }) {
                Some(age) => format!("{spin} updating · {age}"),
                None => format!("{spin} checking"),
            }
        }
        LimitsSectionStatus::Live => String::new(),
        LimitsSectionStatus::Observed { observed_at_unix } => {
            match crate::claude_runtime::rate_limit::format_age_since(observed_at_unix, now_unix) {
                Some(age) => format!("last seen {age}"),
                None => String::new(),
            }
        }
        LimitsSectionStatus::Failed { cached_at_unix } => match cached_at_unix {
            Some(_) => "update failed".into(),
            None => "unavailable".into(),
        },
        LimitsSectionStatus::Empty => String::new(),
    }
}

fn placeholder_window_line(label_width: usize, width: usize) -> Line<'static> {
    let prefix = format!("  {:label_width$}   ", "—");
    let bar = "░".repeat(BAR_WIDTH);
    let text = format!("{prefix}{bar}");
    Line::from(Span::styled(truncate_to(text, width), Theme::dim()))
}

fn usage_limit_window_lines(
    window: &UsageLimitWindow,
    label_width: usize,
    width: usize,
    now: i64,
    block_style: Style,
) -> Vec<Line<'static>> {
    let reset = window
        .resets_at_unix
        .map(|resets_at| format!("resets {}", format_reset_at(resets_at, now)));
    let note = window.note.as_deref();

    if let Some(remaining) = window.remaining_percent {
        return remaining_window_lines(
            &window.label,
            label_width,
            remaining.round().clamp(0.0, 100.0) as u8,
            reset.as_deref(),
            note,
            width,
            block_style,
        );
    }

    let mut detail = String::new();
    if let Some(note) = note {
        detail.push_str(note);
    }
    if let Some(reset) = &reset {
        if !detail.is_empty() {
            detail.push_str("  · ");
        }
        detail.push_str(reset);
    }
    let text = if detail.is_empty() {
        format!("  {:label_width$}", window.label)
    } else {
        format!("  {:label_width$}   {detail}", window.label)
    };
    vec![Line::from(Span::styled(
        truncate_to(text, width),
        block_style,
    ))]
}

fn remaining_window_lines(
    label: &str,
    label_width: usize,
    remaining: u8,
    reset: Option<&str>,
    note: Option<&str>,
    width: usize,
    block_style: Style,
) -> Vec<Line<'static>> {
    let filled = (usize::from(remaining) * BAR_WIDTH + 50) / 100;
    let bar_style = block_style.patch(remaining_style(remaining));
    let prefix = format!("  {label:label_width$}   ");
    let percent = format!("  {remaining}% left");
    let mut suffix = String::new();
    if let Some(reset) = reset {
        suffix.push_str("  · ");
        suffix.push_str(reset);
    }
    if let Some(note) = note {
        suffix.push_str("  · ");
        suffix.push_str(note);
    }
    let show_suffix =
        display_width(&prefix) + BAR_WIDTH + display_width(&percent) + display_width(&suffix)
            <= width;
    let main_line = Line::from(vec![
        Span::styled(prefix, block_style),
        Span::styled("█".repeat(filled), bar_style),
        Span::styled(
            "░".repeat(BAR_WIDTH - filled),
            block_style.patch(Theme::dim()),
        ),
        Span::styled(percent, block_style),
        Span::styled(
            if show_suffix {
                suffix.clone()
            } else {
                String::new()
            },
            block_style,
        ),
    ]);
    if show_suffix {
        return vec![main_line];
    }
    let mut lines = vec![main_line];
    if let Some(reset) = reset {
        lines.push(Line::from(Span::styled(
            format!("  {reset}"),
            block_style.patch(Theme::dim()),
        )));
    }
    if let Some(note) = note {
        lines.extend(indented_note_lines(
            note,
            width,
            block_style.patch(Theme::dim()),
        ));
    }
    lines
}

fn indented_note_lines(note: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from(Span::styled("", style))];
    }
    let indent_width = 2.min(width.saturating_sub(1));
    let note_width = width.saturating_sub(indent_width).max(1);
    let indent = " ".repeat(indent_width);
    wrap_line_at_whitespace(note, note_width)
        .into_iter()
        .map(|part| {
            Line::from(Span::styled(
                format!("{indent}{}", part.trim_start()),
                style,
            ))
        })
        .collect()
}

fn remaining_style(remaining: u8) -> Style {
    if remaining > 50 {
        Theme::success()
    } else if remaining >= 20 {
        Theme::warning()
    } else {
        Theme::error()
    }
}

fn format_reset_at(resets_at_unix: i64, now: i64) -> String {
    let seconds = resets_at_unix.saturating_sub(now);
    if seconds <= 0 {
        return "now".into();
    }
    if seconds < RELATIVE_RESET_CUTOFF_SECONDS {
        let hours = seconds / 3600;
        let minutes = seconds % 3600 / 60;
        return if hours > 0 {
            format!("in {hours}h {minutes}m")
        } else {
            format!("in {minutes}m")
        };
    }

    chrono::DateTime::from_timestamp(resets_at_unix, 0)
        .map(|reset| {
            reset
                .with_timezone(&chrono::Local)
                .format("%b %d at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| format!("at Unix time {resets_at_unix}"))
}

fn truncate_to(text: String, width: usize) -> String {
    super::render::truncate_one_line(&text, width.max(1))
}

#[cfg(test)]
#[path = "limits_command_tests.rs"]
mod tests;
