use std::io::{self, IsTerminal, Write};

use crossterm::style::Stylize;
use rho_providers::model::{ModelMetadata, ModelUsage};

use super::{
    session_picker::short_session_id,
    usage_cost::{
        cache_hit_percent, display_input_tokens, format_token_count, format_usd,
        resolved_usage_cost_usd_micros, session_total_cost_usd_micros,
    },
};

/// Facts printed to stdout after the interactive TUI restores the terminal.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExitReceipt {
    session_id: String,
    title: Option<String>,
    total_cost_usd_micros: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_hit_percent: Option<f64>,
}

impl ExitReceipt {
    pub(super) fn capture(
        session_id: String,
        title: Option<String>,
        usage: Option<&ModelUsage>,
        model_metadata: Option<&ModelMetadata>,
        extra_cost_usd_micros: u64,
    ) -> Self {
        let main_cost =
            usage.and_then(|usage| resolved_usage_cost_usd_micros(usage, model_metadata));
        Self {
            session_id,
            title,
            total_cost_usd_micros: session_total_cost_usd_micros(main_cost, extra_cost_usd_micros),
            input_tokens: usage.and_then(display_input_tokens),
            output_tokens: usage.and_then(|usage| usage.output_tokens),
            cache_hit_percent: usage.and_then(cache_hit_percent),
        }
    }
}

pub(crate) fn print_exit_receipt(receipt: Option<&ExitReceipt>) -> io::Result<()> {
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let styled = io::stdout().is_terminal();
    let mut stdout = io::stdout();
    writeln!(stdout, "{}", format_exit_receipt(receipt, styled))?;
    stdout.flush()
}

/// Compact post-TUI session receipt. `styled` dims labels with ANSI.
fn format_exit_receipt(receipt: &ExitReceipt, styled: bool) -> String {
    let short_id = short_session_id(&receipt.session_id);
    let headline = receipt
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&short_id);
    let mut lines = vec![format!("session saved: {headline}")];
    lines.push(format!(
        "  {}  rho --resume {short_id}",
        padded_label("resume", styled)
    ));
    if let Some(usage_line) = format_usage_line(receipt, styled) {
        lines.push(usage_line);
    }
    lines.join("\n")
}

fn format_usage_line(receipt: &ExitReceipt, styled: bool) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(total) = receipt.total_cost_usd_micros {
        parts.push(format_usd(total));
    }
    if let Some(tokens) = format_token_usage(receipt.input_tokens, receipt.output_tokens) {
        parts.push(tokens);
    }
    if let Some(percent) = receipt.cache_hit_percent {
        parts.push(format!("{percent:.0}% cache hit"));
    }
    (!parts.is_empty())
        .then(|| format!("  {}  {}", padded_label("usage", styled), parts.join(" · ")))
}

fn format_token_usage(input: Option<u64>, output: Option<u64>) -> Option<String> {
    match (input, output) {
        (None, None) => None,
        (Some(input), Some(output)) => Some(format!(
            "{} in / {} out",
            format_token_count(input),
            format_token_count(output)
        )),
        (Some(input), None) => Some(format!("{} in", format_token_count(input))),
        (None, Some(output)) => Some(format!("{} out", format_token_count(output))),
    }
}

fn padded_label(name: &str, styled: bool) -> String {
    let padded = format!("{name:<6}");
    if styled {
        padded.dim().to_string()
    } else {
        padded
    }
}

#[cfg(test)]
#[path = "exit_receipt_tests.rs"]
mod tests;
