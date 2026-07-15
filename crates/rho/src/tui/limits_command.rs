use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{theme::Theme, App, Entry};
use crate::usage_limits::{
    fetch_connected_usage_limits, now_unix, ProviderLimits, ProviderUsageLimits, UsageLimitWindow,
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
        match fetch_connected_usage_limits(self.credential_store.as_ref()).await {
            Ok((limits, errors)) if limits.providers.is_empty() && errors.is_empty() => {
                self.insert_entry(&Entry::Notice(
                    "no supported OAuth providers are connected; connect Codex with /login openai-codex or xAI with /login xai"
                        .into(),
                ));
                self.status = "no supported OAuth providers connected".into();
            }
            Ok((limits, errors))
                if limits
                    .providers
                    .iter()
                    .all(|provider| provider.windows.is_empty())
                    && errors.is_empty() =>
            {
                let names = provider_names(&limits);
                self.insert_entry(&Entry::Notice(format!(
                    "{names} did not report any active usage limit windows"
                )));
                self.status = "no OAuth usage limits reported".into();
            }
            Ok((limits, errors)) => {
                self.insert_entry(&Entry::UsageLimits(limits));
                for error in &errors {
                    self.insert_entry(&Entry::Error(format!(
                        "could not check OAuth usage limits: {error}"
                    )));
                }
                self.status = if errors.is_empty() {
                    "OAuth usage limits updated".into()
                } else {
                    "OAuth usage limits partially updated".into()
                };
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not check OAuth usage limits: {error}"
                )));
                self.status = "OAuth usage limit check failed".into();
            }
        }
        Ok(())
    }
}

pub(super) fn usage_limit_lines(limits: &ProviderLimits, width: usize) -> Vec<Line<'static>> {
    let now = now_unix();
    let block_style = Theme::limits_block();
    let mut lines = vec![
        Line::from(Span::styled(
            "OAuth usage limits",
            block_style.add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(Span::styled("", block_style)),
    ];
    for (index, provider) in limits.providers.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(Span::styled("", block_style)));
        }
        lines.extend(provider_usage_limit_lines(
            provider,
            width,
            now,
            block_style,
        ));
    }
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

fn provider_names(limits: &ProviderLimits) -> String {
    let names = limits
        .providers
        .iter()
        .map(|provider| provider.provider.as_str())
        .collect::<Vec<_>>();
    match names.as_slice() {
        [] => "Connected providers".into(),
        [name] => (*name).into(),
        [first, second] => format!("{first} and {second}"),
        [first, second, third] => format!("{first}, {second}, and {third}"),
        _ => {
            let (last, rest) = names.split_last().expect("non-empty names");
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

#[cfg(test)]
#[path = "limits_command_tests.rs"]
mod tests;
