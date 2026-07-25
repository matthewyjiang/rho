use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{command_block::fill_lines, render::wrap_line_at_whitespace, theme::Theme, App, Entry};
use crate::usage_limits::{
    fetch_connected_usage_limits, now_unix, ProviderLimits, ProviderUsageLimits, UsageLimitWindow,
    UsageLimitsError,
};

const BAR_WIDTH: usize = 10;
const RELATIVE_RESET_CUTOFF_SECONDS: i64 = 24 * 60 * 60;

pub(super) type LimitsFetchResult =
    Result<(ProviderLimits, Vec<UsageLimitsError>), UsageLimitsError>;

impl App {
    pub(super) fn execute_limits_command(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.start_limits_command() {
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    pub(super) fn start_limits_command(&mut self) -> bool {
        if self.pending_usage_limits.is_some() {
            self.insert_entry(&Entry::Notice(
                "an OAuth usage limit check is already in progress".into(),
            ));
            self.status = "checking OAuth usage limits".into();
            return false;
        }

        let credential_store = self.credential_store.clone();
        let client = self.usage_limits_client.clone();
        self.pending_usage_limits = Some(tokio::spawn(async move {
            fetch_connected_usage_limits(credential_store.as_ref(), client).await
        }));
        self.status = "checking OAuth usage limits".into();
        true
    }

    pub(super) async fn cancel_limits_command(&mut self) {
        if let Some(handle) = self.pending_usage_limits.take() {
            handle.abort();
            let _ = handle.await;
        }
    }

    pub(super) async fn poll_limits_command(&mut self) -> anyhow::Result<bool> {
        if !self
            .pending_usage_limits
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(false);
        }
        self.finish_limits_command().await?;
        Ok(true)
    }

    async fn finish_limits_command(&mut self) -> anyhow::Result<()> {
        let Some(handle) = self.pending_usage_limits.take() else {
            return Ok(());
        };
        match handle.await {
            Ok(result) => self.render_limits_result(result),
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not check OAuth usage limits: background task failed: {error}"
                )));
                self.status = "OAuth usage limit check failed".into();
            }
        }
        Ok(())
    }

    fn render_limits_result(&mut self, result: LimitsFetchResult) {
        // `/limits` never probes Claude. It only shows windows a prior
        // claude-cli run persisted, or that none are known yet.
        let view = present_limits_result(
            result,
            crate::claude_runtime::rate_limit::load().as_ref(),
            crate::claude_runtime::rate_limit::now_unix(),
        );
        for item in view.items {
            match item {
                LimitsViewItem::UsageLimits(limits) => {
                    self.insert_entry(&Entry::UsageLimits(limits));
                }
                LimitsViewItem::Error(text) => self.insert_entry(&Entry::Error(text)),
            }
        }
        self.status = view.status;
    }
}

/// First-class `/limits` block: every source as a provider section.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LimitsDisplay {
    pub(super) providers: Vec<ProviderUsageLimits>,
    /// Empty-state copy when no provider reported usable windows.
    pub(super) empty_note: Option<String>,
}

/// One history row produced by `/limits` presentation.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum LimitsViewItem {
    UsageLimits(LimitsDisplay),
    Error(String),
}

/// Pure `/limits` presentation: OAuth fetch result plus optional Claude cache.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LimitsView {
    pub(super) items: Vec<LimitsViewItem>,
    pub(super) status: String,
}

/// Build `/limits` history rows without I/O.
///
/// Claude's cache is normalized into [`ProviderUsageLimits`] so one renderer
/// owns every source. Remaining percent is shown only when reported.
pub(super) fn present_limits_result(
    result: LimitsFetchResult,
    claude: Option<&crate::claude_runtime::rate_limit::RateLimitState>,
    now_unix: i64,
) -> LimitsView {
    let claude_provider = claude_provider_limits(claude, now_unix);
    let has_claude = claude_provider.is_some();
    match result {
        Ok((mut limits, errors)) => {
            if let Some(claude_provider) = claude_provider {
                limits.providers.push(claude_provider);
            }
            let empty_note = empty_note_for(&limits);
            let status = status_for(&limits, &errors, has_claude);
            let mut items = vec![LimitsViewItem::UsageLimits(LimitsDisplay {
                providers: limits.providers,
                empty_note,
            })];
            for error in &errors {
                items.push(LimitsViewItem::Error(format!(
                    "could not check OAuth usage limits: {error}"
                )));
            }
            LimitsView { items, status }
        }
        Err(error) => LimitsView {
            items: vec![
                LimitsViewItem::UsageLimits(LimitsDisplay {
                    providers: claude_provider.into_iter().collect(),
                    empty_note: None,
                }),
                LimitsViewItem::Error(format!("could not check OAuth usage limits: {error}")),
            ],
            status: "OAuth usage limit check failed".into(),
        },
    }
}

fn claude_provider_limits(
    state: Option<&crate::claude_runtime::rate_limit::RateLimitState>,
    now_unix: i64,
) -> Option<ProviderUsageLimits> {
    let state = state.filter(|state| !state.is_empty())?;
    let windows = state
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
            note_parts.push(format!(
                "observed {}",
                crate::claude_runtime::rate_limit::format_age(window.age_seconds(now_unix))
            ));
            UsageLimitWindow {
                label: window.info.window_label(),
                remaining_percent: window.info.remaining_percent(),
                resets_at_unix: window.info.resets_at,
                note: Some(note_parts.join(", ")),
            }
        })
        .collect();
    Some(ProviderUsageLimits {
        provider: "Claude Code".into(),
        windows,
    })
}

fn empty_note_for(limits: &ProviderLimits) -> Option<String> {
    if limits.providers.is_empty() {
        return Some(
            "no supported OAuth providers are connected and no Claude Code limits are known yet; connect Codex with /login openai-codex, Kimi Code with /login kimi-code, xAI with /login xai-oauth, or run a claude-cli agent"
                .into(),
        );
    }
    if limits
        .providers
        .iter()
        .all(|provider| provider.windows.is_empty())
    {
        let names = provider_names(limits);
        return Some(format!(
            "{names} did not report any active usage limit windows"
        ));
    }
    None
}

fn status_for(limits: &ProviderLimits, errors: &[UsageLimitsError], has_claude: bool) -> String {
    if !errors.is_empty() {
        return "OAuth usage limits partially updated".into();
    }
    if limits.providers.is_empty() {
        return "no usage limits available".into();
    }
    if limits
        .providers
        .iter()
        .all(|provider| provider.windows.is_empty())
    {
        return "no usage limits reported".into();
    }
    if has_claude
        && limits
            .providers
            .iter()
            .all(|provider| provider.provider == "Claude Code")
    {
        return "claude code limits only".into();
    }
    "usage limits updated".into()
}

pub(super) fn usage_limit_lines(limits: &LimitsDisplay, width: usize) -> Vec<Line<'static>> {
    let now = now_unix();
    let block_style = Theme::command_block();
    let mut lines = vec![Line::from(Span::styled(
        "Usage limits",
        block_style.add_modifier(ratatui::style::Modifier::BOLD),
    ))];

    if let Some(note) = &limits.empty_note {
        lines.push(Line::from(Span::styled("", block_style)));
        lines.extend(indented_note_lines(note, width, block_style));
    }

    for provider in &limits.providers {
        if limits.empty_note.is_some() && provider.windows.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled("", block_style)));
        lines.extend(provider_usage_limit_lines(
            provider,
            width,
            now,
            block_style,
        ));
    }

    fill_lines(lines, width, block_style)
}

fn provider_usage_limit_lines(
    provider: &ProviderUsageLimits,
    width: usize,
    now: i64,
    block_style: Style,
) -> Vec<Line<'static>> {
    let label_width = provider
        .windows
        .iter()
        .map(|window| window.label.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = vec![Line::from(Span::styled(
        provider.provider.clone(),
        block_style.add_modifier(ratatui::style::Modifier::BOLD),
    ))];
    if provider.windows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no active usage limit windows reported",
            block_style.patch(Theme::dim()),
        )));
        return lines;
    }
    lines.extend(
        provider.windows.iter().flat_map(|window| {
            usage_limit_window_lines(window, label_width, width, now, block_style)
        }),
    );
    lines
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
    vec![Line::from(Span::styled(text, block_style))]
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
        prefix.chars().count() + BAR_WIDTH + percent.chars().count() + suffix.chars().count()
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
        lines.extend(indented_note_lines(note, width, block_style));
    }
    lines
}

fn indented_note_lines(note: &str, width: usize, block_style: Style) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from(Span::styled("", block_style))];
    }
    let indent_width = 2.min(width.saturating_sub(1));
    let note_width = width.saturating_sub(indent_width).max(1);
    let indent = " ".repeat(indent_width);
    wrap_line_at_whitespace(note, note_width)
        .into_iter()
        .map(|part| {
            Line::from(Span::styled(
                format!("{indent}{}", part.trim_start()),
                block_style.patch(Theme::dim()),
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

fn provider_names(limits: &ProviderLimits) -> String {
    let names: Vec<&str> = limits
        .providers
        .iter()
        .map(|provider| provider.provider.as_str())
        .collect();
    match names.as_slice() {
        [] => "connected providers".into(),
        [one] => (*one).into(),
        [first, second] => format!("{first} and {second}"),
        [first, second, rest @ ..] => {
            format!("{first}, {second}, and {}", rest.join(", "))
        }
    }
}

#[cfg(test)]
#[path = "limits_command_tests.rs"]
mod tests;
