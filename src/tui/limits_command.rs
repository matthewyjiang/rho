use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{theme::Theme, App, Entry};
use crate::usage_limits::{
    now_unix, CodexUsageLimitsSource, ProviderLimits, UsageLimitWindow, UsageLimitsSource,
};

const BAR_WIDTH: usize = 10;
const RELATIVE_RESET_CUTOFF_SECONDS: i64 = 24 * 60 * 60;

impl App {
    pub(super) async fn execute_limits_command(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.status = "checking OAuth usage limits".into();
        terminal.draw(|frame| self.draw(frame))?;
        let source = CodexUsageLimitsSource::new();
        match source.fetch(self.credential_store.as_ref()).await {
            Ok(None) => {
                self.insert_entry(&Entry::Notice(
                    "no supported OAuth providers are connected; connect Codex with /login openai-codex"
                        .into(),
                ));
                self.status = "no supported OAuth providers connected".into();
            }
            Ok(Some(limits)) if limits.windows.is_empty() => {
                self.insert_entry(&Entry::Notice(
                    "Codex did not report any active usage limit windows".into(),
                ));
                self.status = "no OAuth usage limits reported".into();
            }
            Ok(Some(limits)) => {
                self.insert_entry(&Entry::UsageLimits(limits));
                self.status = "OAuth usage limits updated".into();
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not check Codex OAuth usage limits: {error}"
                )));
                self.status = "OAuth usage limit check failed".into();
            }
        }
        Ok(())
    }
}

pub(super) fn usage_limit_lines(limits: &ProviderLimits, width: usize) -> Vec<Line<'static>> {
    let label_width = limits
        .windows
        .iter()
        .map(|window| window.label.chars().count())
        .max()
        .unwrap_or(0);
    let now = now_unix();
    let mut lines = vec![
        Line::styled("OAuth usage limits", Theme::text_strong()),
        Line::raw(""),
        Line::styled(limits.provider.clone(), Theme::text_strong()),
    ];
    lines.extend(
        limits
            .windows
            .iter()
            .map(|window| usage_limit_line(window, label_width, width, now)),
    );
    lines
}

fn usage_limit_line(
    window: &UsageLimitWindow,
    label_width: usize,
    width: usize,
    now: i64,
) -> Line<'static> {
    let remaining = window.remaining_percent.round() as u8;
    let filled = (usize::from(remaining) * BAR_WIDTH + 50) / 100;
    let bar_style = remaining_style(remaining);
    let reset = format!("resets {}", format_reset(window, now));
    let prefix = format!("  {:label_width$}   ", window.label);
    let percent = format!("  {remaining}% left");
    let reset_suffix = format!("  · {reset}");
    let show_reset =
        prefix.chars().count() + BAR_WIDTH + percent.chars().count() + reset_suffix.chars().count()
            <= width;
    let mut spans = vec![
        Span::raw(prefix),
        Span::styled("█".repeat(filled), bar_style),
        Span::styled("░".repeat(BAR_WIDTH - filled), Theme::dim()),
        Span::raw(percent),
    ];
    if show_reset {
        spans.push(Span::raw(reset_suffix));
    }
    Line::from(spans)
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

fn format_reset(window: &UsageLimitWindow, now: i64) -> String {
    let seconds = window.resets_at_unix.saturating_sub(now);
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

    chrono::DateTime::from_timestamp(window.resets_at_unix, 0)
        .map(|reset| {
            reset
                .with_timezone(&chrono::Local)
                .format("%a at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| format!("at Unix time {}", window.resets_at_unix))
}

#[cfg(test)]
#[path = "limits_command_tests.rs"]
mod tests;
