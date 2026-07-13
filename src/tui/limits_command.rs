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
    let block_style = Theme::limits_block();
    let mut lines = vec![
        Line::from(Span::styled(
            "OAuth usage limits",
            block_style.add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(Span::styled("", block_style)),
        Line::from(Span::styled(
            limits.provider.clone(),
            block_style.add_modifier(ratatui::style::Modifier::BOLD),
        )),
    ];
    lines.extend(
        limits.windows.iter().flat_map(|window| {
            usage_limit_window_lines(window, label_width, width, now, block_style)
        }),
    );
    lines
        .into_iter()
        .map(|mut line| {
            let padding = width.saturating_sub(line.width());
            if padding > 0 {
                line.spans
                    .push(Span::styled(" ".repeat(padding), block_style));
            }
            line
        })
        .collect()
}

fn usage_limit_window_lines(
    window: &UsageLimitWindow,
    label_width: usize,
    width: usize,
    now: i64,
    block_style: Style,
) -> Vec<Line<'static>> {
    let remaining = window.remaining_percent.round() as u8;
    let filled = (usize::from(remaining) * BAR_WIDTH + 50) / 100;
    let bar_style = block_style.patch(remaining_style(remaining));
    let reset = format!("resets {}", format_reset(window, now));
    let prefix = format!("  {:label_width$}   ", window.label);
    let percent = format!("  {remaining}% left");
    let reset_suffix = format!("  · {reset}");
    let show_reset =
        prefix.chars().count() + BAR_WIDTH + percent.chars().count() + reset_suffix.chars().count()
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
            if show_reset {
                reset_suffix.clone()
            } else {
                String::new()
            },
            block_style,
        ),
    ]);
    if show_reset {
        vec![main_line]
    } else {
        vec![
            main_line,
            Line::from(Span::styled(
                format!("  {reset}"),
                block_style.patch(Theme::dim()),
            )),
        ]
    }
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
                .format("%b %d at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| format!("at Unix time {}", window.resets_at_unix))
}

#[cfg(test)]
#[path = "limits_command_tests.rs"]
mod tests;
