use std::path::PathBuf;

use ratatui::text::{Line, Span};
use rho_providers::model::{ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage};

use super::{
    render::{truncate_one_line, wrap_line_at_whitespace},
    statusline::{cache_hit_percent, estimated_cost_usd_micros, git_branch, BillingInfo},
    theme::Theme,
    App, Entry,
};

const LABEL_WIDTH: usize = 14;

#[derive(Clone, Debug)]
pub(super) struct RuntimeInfo {
    version: String,
    provider: String,
    model: String,
    reasoning: String,
    permission_mode: String,
    billing: BillingInfo,
    cwd: PathBuf,
    branch: Option<String>,
    usage: Option<ModelUsage>,
    latest_usage: Option<ModelUsage>,
    context_usage: Option<ContextUsage>,
    model_metadata: Option<ModelMetadata>,
}

impl App {
    pub(super) fn execute_info_command(&mut self) -> anyhow::Result<()> {
        let identity = self.info.services.diagnostics.identity();
        let info = RuntimeInfo {
            version: identity.rho_version.to_string(),
            provider: identity.provider.to_string(),
            model: identity.model.to_string(),
            reasoning: identity.reasoning.to_string(),
            permission_mode: self.info.runtime.permission_mode.as_str().into(),
            billing: BillingInfo::from_provider_auth(
                &self.info.runtime.provider,
                &self.info.runtime.auth,
            ),
            cwd: self.info.runtime.cwd.clone(),
            branch: git_branch(&self.info.runtime.cwd),
            usage: self.cumulative_usage.clone(),
            latest_usage: self.latest_usage.clone(),
            context_usage: self.current_context.clone(),
            model_metadata: self.model_metadata.clone(),
        };
        self.insert_entry(&Entry::RuntimeInfo(Box::new(info)));
        self.status = "runtime info".into();
        Ok(())
    }
}

pub(super) fn runtime_info_lines(info: &RuntimeInfo, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled("rho", Theme::brand()),
        Span::raw("  "),
        Span::styled(format!("v{}", info.version), Theme::dim()),
    ])];

    push_section(&mut lines, "Model");
    push_field(&mut lines, "Provider", &info.provider, width);
    push_field(&mut lines, "Model", &info.model, width);
    push_field(&mut lines, "Reasoning", &info.reasoning, width);
    push_field(&mut lines, "Permissions", &info.permission_mode, width);
    push_field(&mut lines, "Billing", info.billing.description(), width);

    push_section(&mut lines, "Session usage");
    push_usage_fields(&mut lines, info, width);

    push_section(&mut lines, "Workspace");
    push_field(
        &mut lines,
        "Directory",
        &info.cwd.display().to_string(),
        width,
    );
    push_field(
        &mut lines,
        "Git branch",
        info.branch.as_deref().unwrap_or("not in a Git worktree"),
        width,
    );
    lines
}

fn push_section(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::raw(""));
    lines.push(Line::styled(title.to_string(), Theme::text_strong()));
}

fn push_usage_fields(lines: &mut Vec<Line<'static>>, info: &RuntimeInfo, width: usize) {
    if let Some(context) = format_context(info) {
        push_field(lines, "Context", &context, width);
    } else {
        push_field(lines, "Context", "not reported", width);
    }

    let Some(usage) = info.usage.as_ref() else {
        push_note(lines, "No token usage recorded yet.", width);
        return;
    };

    push_optional_number(lines, "Input tokens", usage.input_tokens, width);
    push_optional_number(lines, "Output tokens", usage.output_tokens, width);
    push_optional_number(lines, "Cache read", usage.cache_read_tokens, width);
    push_optional_number(lines, "Cache write", usage.cache_write_tokens, width);
    if let Some(percent) = cache_hit_percent(info.latest_usage.as_ref()) {
        push_field(
            lines,
            "Cache hit",
            &format!("{percent:.1}% on the latest request"),
            width,
        );
    }

    let reported_cost = usage.cost_usd_micros;
    let cost =
        reported_cost.or_else(|| estimated_cost_usd_micros(usage, info.model_metadata.as_ref()));
    if let Some(cost) = cost {
        let qualifier = if reported_cost.is_none() {
            " estimated"
        } else {
            ""
        };
        let equivalent = if info.billing == BillingInfo::Subscription {
            " API equivalent"
        } else {
            ""
        };
        push_field(
            lines,
            "Cost",
            &format!("{}{qualifier}{equivalent}", format_usd(cost)),
            width,
        );
    }
}

fn push_optional_number(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: Option<u64>,
    width: usize,
) {
    if let Some(value) = value {
        push_field(lines, label, &format_number(value), width);
    }
}

fn push_field(lines: &mut Vec<Line<'static>>, label: &str, value: &str, width: usize) {
    let label_width = LABEL_WIDTH.min(width.saturating_sub(1));
    let value_start = label_width.saturating_add(2);
    if width >= 32 && value_start < width {
        let mut values = wrap_line_at_whitespace(value, width - value_start).into_iter();
        let first = values.next().unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(format!("  {label:label_width$}"), Theme::dim()),
            Span::styled(first, Theme::text()),
        ]));
        lines.extend(values.map(|value| {
            Line::from(vec![
                Span::raw(" ".repeat(value_start)),
                Span::styled(value.trim_start().to_string(), Theme::text()),
            ])
        }));
    } else {
        lines.push(Line::from(Span::styled(
            truncate_one_line(&format!("  {label}"), width),
            Theme::dim(),
        )));
        let indent_width = 4.min(width.saturating_sub(1));
        let value_width = width.saturating_sub(indent_width).max(1);
        let indent = " ".repeat(indent_width);
        lines.extend(
            wrap_line_at_whitespace(value, value_width)
                .into_iter()
                .map(|value| {
                    Line::from(Span::styled(
                        format!("{indent}{}", value.trim_start()),
                        Theme::text(),
                    ))
                }),
        );
    }
}

fn push_note(lines: &mut Vec<Line<'static>>, note: &str, width: usize) {
    let indent_width = 2.min(width.saturating_sub(1));
    let note_width = width.saturating_sub(indent_width).max(1);
    let indent = " ".repeat(indent_width);
    lines.extend(
        wrap_line_at_whitespace(note, note_width)
            .into_iter()
            .map(|part| {
                Line::from(Span::styled(
                    format!("{indent}{}", part.trim_start()),
                    Theme::dim_italic(),
                ))
            }),
    );
}

fn format_context(info: &RuntimeInfo) -> Option<String> {
    let window = info
        .context_usage
        .as_ref()
        .and_then(|usage| usage.context_window)
        .or_else(|| {
            info.model_metadata
                .as_ref()
                .and_then(ModelMetadata::display_context_window)
        })
        .filter(|window| *window > 0)?;
    let source = match info.context_usage.as_ref().map(|usage| usage.source) {
        Some(ContextUsageSource::Estimated) => "estimated",
        Some(ContextUsageSource::ProviderReported) => "provider reported",
        Some(ContextUsageSource::UnknownAfterCompaction) => "unknown after compaction",
        None => "model limit",
    };
    let Some(tokens) = info.context_usage.as_ref().and_then(|usage| usage.tokens) else {
        return Some(format!(
            "unknown / {} tokens ({source})",
            format_number(window)
        ));
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    Some(format!(
        "{} / {} tokens ({percent:.1}%, {source})",
        format_number(tokens),
        format_number(window)
    ))
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(ch);
    }
    formatted
}

fn format_usd(micros: u64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    if dollars >= 100.0 {
        format!("${dollars:.0}")
    } else if dollars >= 10.0 {
        format!("${dollars:.2}")
    } else {
        format!("${dollars:.3}")
    }
}

#[cfg(test)]
#[path = "info_command_tests.rs"]
mod tests;
