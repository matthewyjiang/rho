//! Header, footer, and usage chrome for the attach view.

use ratatui::text::{Line, Span};
use rho_sdk::model::{ContextUsage, ModelUsage};

use super::super::{
    render::truncate_one_line,
    theme::Theme,
    usage_cost::{
        context_fill_percent, format_token_count, format_usage_token_summary, format_usd,
        resolved_context_window, resolved_usage_cost_usd_micros,
    },
};
use crate::{
    herdr::HerdrState,
    subagent::{self, RunState, RunStatus},
};

/// Separator between attach header fields. Matches the main TUI statusline.
const FIELD_SEP: &str = " · ";

/// Host-specific attach chrome. Footer copy and parent badges live here so
/// `AttachmentApp` stays a viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachChrome {
    Standalone,
    Embedded { notice: Option<ParentNotice> },
}

/// Parent session state shown in the embedded attach footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParentNotice {
    Approval,
    Questionnaire,
    TurnComplete,
}

impl AttachChrome {
    fn footer_hint(self) -> &'static str {
        match self {
            Self::Standalone => "read-only · scroll · ctrl+o expand · q detach",
            Self::Embedded { .. } => "read-only · scroll · tab cycle · ctrl+o expand · q back",
        }
    }

    fn parent_notice(self) -> Option<ParentNotice> {
        match self {
            Self::Standalone => None,
            Self::Embedded { notice } => notice,
        }
    }
}

impl ParentNotice {
    fn label(self) -> &'static str {
        match self {
            Self::Approval => "parent approval waiting",
            Self::Questionnaire => "parent questionnaire waiting",
            Self::TurnComplete => "parent turn complete",
        }
    }
}

pub(super) fn footer_line(chrome: AttachChrome, width: usize) -> Line<'static> {
    let hint = chrome.footer_hint();
    let text = match chrome.parent_notice() {
        Some(notice) => format!("{}{FIELD_SEP}{hint}", notice.label()),
        None => hint.to_string(),
    };
    Line::styled(truncate_one_line(&text, width), Theme::dim())
}

/// Middle header row: model, runtime, turn, elapsed, optional Claude session, cost.
pub(super) fn identity_line(
    status: Option<&RunStatus>,
    run_usage: Option<&ModelUsage>,
    now_unix_secs: u64,
) -> String {
    let Some(status) = status else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(model) =
        crate::model_identity::PromptModel::from_run_status(status).map(|model| model.describe())
    {
        parts.push(model);
    }
    if let Some(runtime) = status.runtime {
        parts.push(runtime.as_str().to_string());
    }
    parts.push(format!("turn {}", status.turns));
    if let Some(elapsed) = status
        .elapsed_duration(now_unix_secs)
        .map(|elapsed| subagent::format_elapsed_secs(elapsed.as_secs()))
    {
        parts.push(elapsed);
    }
    if let Some(session_id) = status
        .claude_session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
    {
        parts.push(format!("claude {session_id}"));
    }
    if let Some(cost) = format_run_cost(status, run_usage) {
        parts.push(cost);
    }
    join_fields(parts)
}

/// Bottom header row: what the run is doing plus live usage.
pub(super) fn activity_metrics_line(
    activity: &str,
    context: Option<&ContextUsage>,
    run_usage: Option<&ModelUsage>,
    status: Option<&RunStatus>,
    average_generation_rate: Option<u64>,
) -> String {
    let mut parts = vec![activity.to_string()];
    parts.extend(usage_metric_parts(context, run_usage, status));
    if let Some(rate) = average_generation_rate {
        parts.push(format!("{rate} tok/s"));
    }
    join_fields(parts)
}

fn usage_metric_parts(
    context: Option<&ContextUsage>,
    run_usage: Option<&ModelUsage>,
    status: Option<&RunStatus>,
) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(context_summary) = format_attach_context(context) {
        parts.push(context_summary);
    }
    if let Some(usage_summary) = run_usage
        .and_then(format_usage_token_summary)
        .or_else(|| status.and_then(|status| format_usage_token_summary(&run_status_usage(status))))
    {
        parts.push(usage_summary);
    }
    parts
}

pub(super) fn header_title_line(
    run_id: &str,
    agent_id: &str,
    state: &str,
    status: Option<&RunStatus>,
) -> Line<'static> {
    Line::from(vec![
        Span::styled("rho", Theme::brand()),
        Span::raw(format!("  attach {run_id}")),
        Span::styled(format!(" · {agent_id}"), Theme::dim()),
        Span::styled(format!(" · {state}"), state_style(status)),
    ])
}

fn join_fields(parts: Vec<String>) -> String {
    parts.join(FIELD_SEP)
}

fn run_status_usage(status: &RunStatus) -> ModelUsage {
    ModelUsage {
        input_tokens: status.input_tokens,
        output_tokens: status.output_tokens,
        ..ModelUsage::default()
    }
}

fn format_attach_context(context: Option<&ContextUsage>) -> Option<String> {
    let context = context?;
    let tokens = context.tokens?;
    // Attach has no model metadata, so only the usage-reported window applies.
    match resolved_context_window(Some(context), None) {
        Some(window) => {
            let percent = context_fill_percent(tokens, window);
            Some(format!(
                "context {}/{} ({percent:.1}%)",
                format_token_count(tokens),
                format_token_count(window)
            ))
        }
        None => Some(format!("context {}", format_token_count(tokens))),
    }
}

pub(super) fn format_run_cost(
    status: &RunStatus,
    run_usage: Option<&ModelUsage>,
) -> Option<String> {
    if let Some(cost) = status.total_cost_usd {
        return Some(format_usd(subagent::usd_to_micros(cost)));
    }
    // Attach has no model metadata, so this resolves provider-reported cost only.
    run_usage
        .and_then(|usage| resolved_usage_cost_usd_micros(usage, None))
        .map(format_usd)
}

pub(super) fn herdr_status(id: &str, status: &RunStatus) -> (HerdrState, String) {
    let state = match status.state {
        RunState::Starting | RunState::Running => HerdrState::Working,
        RunState::Error => HerdrState::Blocked,
        RunState::Ok | RunState::Stopped => HerdrState::Idle,
    };
    let detail = status
        .last_activity
        .as_deref()
        .unwrap_or_else(|| status.state.as_str());
    (state, format!("agent run {id}: {detail}"))
}

pub(super) fn state_style(status: Option<&RunStatus>) -> ratatui::style::Style {
    match status.map(|status| status.state) {
        Some(RunState::Ok) => Theme::success(),
        Some(RunState::Error | RunState::Stopped) => Theme::error(),
        Some(RunState::Starting | RunState::Running) | None => Theme::warning(),
    }
}
