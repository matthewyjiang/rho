use std::{path::PathBuf, time::Duration};

use ratatui::text::{Line, Span};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
    usage_cost::{
        format_token_count, format_usd, resolved_usage_cost_usd_micros,
        session_total_cost_usd_micros,
    },
    workspace::git_branch,
    RuntimeModelView,
};
use {
    crate::permission::PermissionMode,
    rho_providers::model::{
        ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage, ReasoningCapabilities,
    },
    rho_providers::reasoning::ReasoningLevel,
};

#[path = "statusline_path.rs"]
mod path;
use path::{compact_cwd, fit_cwd, format_cwd_left};

#[cfg(test)]
use path::shorten_path_display;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusLineState {
    cwd: PathBuf,
    branch: Option<String>,
    usage: Option<ModelUsage>,
    context_usage: Option<ContextUsage>,
    provider: String,
    model: String,
    fast_mode_active: bool,
    reasoning: ReasoningLevel,
    reasoning_configurable: bool,
    permission_mode: PermissionMode,
    model_metadata: Option<ModelMetadata>,
    subagent_total_cost_usd_micros: u64,
    average_output_rate: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StatusLineCache {
    width: usize,
    goal: Option<GoalStatus>,
    lines: Vec<Line<'static>>,
    #[cfg(test)]
    render_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct StatusLine {
    state: StatusLineState,
    cache: StatusLineCache,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GoalStatus {
    pub(super) turns: usize,
    pub(super) elapsed: Duration,
    pub(super) blocked: bool,
}

impl Default for StatusLineState {
    fn default() -> Self {
        Self {
            cwd: PathBuf::new(),
            branch: None,
            usage: None,
            context_usage: None,
            provider: String::new(),
            model: String::new(),
            fast_mode_active: false,
            reasoning: ReasoningLevel::default(),
            reasoning_configurable: true,
            permission_mode: PermissionMode::default(),
            model_metadata: None,
            subagent_total_cost_usd_micros: 0,
            average_output_rate: None,
        }
    }
}

impl StatusLineState {
    fn from_tui(info: &RuntimeModelView) -> Self {
        Self {
            cwd: info.cwd.clone(),
            branch: git_branch(&info.cwd),
            usage: None,
            context_usage: None,
            provider: info.provider.clone(),
            model: info.model.clone(),
            fast_mode_active: info.fast_mode_active(),
            reasoning: info.reasoning,
            reasoning_configurable: reasoning_is_configurable(&info.provider, &info.model),
            permission_mode: info.permission_mode,
            model_metadata: None,
            subagent_total_cost_usd_micros: 0,
            average_output_rate: None,
        }
    }
}

impl StatusLine {
    pub(super) fn new(info: &RuntimeModelView) -> Self {
        Self {
            state: StatusLineState::from_tui(info),
            cache: StatusLineCache::default(),
        }
    }

    pub(super) fn refresh_git_branch(&mut self) {
        let branch = git_branch(&self.state.cwd);
        if self.state.branch != branch {
            self.state.branch = branch;
            self.invalidate();
        }
    }

    pub(super) fn update_model(&mut self, info: &RuntimeModelView) {
        let reasoning_configurable = reasoning_is_configurable(&info.provider, &info.model);
        let fast_mode_active = info.fast_mode_active();
        if self.state.provider != info.provider
            || self.state.model != info.model
            || self.state.fast_mode_active != fast_mode_active
            || self.state.reasoning != info.reasoning
            || self.state.reasoning_configurable != reasoning_configurable
            || self.state.permission_mode != info.permission_mode
        {
            self.state.provider.clone_from(&info.provider);
            self.state.model.clone_from(&info.model);
            self.state.fast_mode_active = fast_mode_active;
            self.state.reasoning = info.reasoning;
            self.state.reasoning_configurable = reasoning_configurable;
            self.state.permission_mode = info.permission_mode;
            self.invalidate();
        }
    }

    pub(super) fn update_usage(
        &mut self,
        usage: Option<&ModelUsage>,
        context_usage: Option<&ContextUsage>,
        subagent_total_cost_usd_micros: u64,
    ) {
        if self.state.usage.as_ref() != usage
            || self.state.context_usage.as_ref() != context_usage
            || self.state.subagent_total_cost_usd_micros != subagent_total_cost_usd_micros
        {
            self.state.usage = usage.cloned();
            self.state.context_usage = context_usage.cloned();
            self.state.subagent_total_cost_usd_micros = subagent_total_cost_usd_micros;
            self.invalidate();
        }
    }

    pub(super) fn update_average_output_rate(&mut self, average_output_rate: Option<u64>) {
        if self.state.average_output_rate != average_output_rate {
            self.state.average_output_rate = average_output_rate;
            self.invalidate();
        }
    }

    pub(super) fn update_model_metadata(&mut self, model_metadata: Option<&ModelMetadata>) {
        let reasoning_configurable =
            reasoning_is_configurable(&self.state.provider, &self.state.model);
        if self.state.model_metadata.as_ref() != model_metadata
            || self.state.reasoning_configurable != reasoning_configurable
        {
            self.state.model_metadata = model_metadata.cloned();
            self.state.reasoning_configurable = reasoning_configurable;
            self.invalidate();
        }
    }

    pub(super) fn lines(&mut self, width: usize, goal: Option<GoalStatus>) -> &[Line<'static>] {
        if self.cache.lines.is_empty() || self.cache.width != width || self.cache.goal != goal {
            let lines = statusline_lines(&self.state, width, goal.as_ref());
            self.cache.width = width;
            self.cache.goal = goal;
            self.cache.lines = lines;
            #[cfg(test)]
            {
                self.cache.render_count += 1;
            }
        }
        &self.cache.lines
    }

    #[cfg(test)]
    pub(super) fn render_count(&self) -> usize {
        self.cache.render_count
    }

    pub(super) fn height(&self) -> usize {
        2
    }

    fn invalidate(&mut self) {
        self.cache.lines.clear();
    }
}

fn reasoning_is_configurable(provider: &str, model: &str) -> bool {
    rho_providers::model::models_dev::current_reasoning_capabilities(provider, model)
        != ReasoningCapabilities::NotConfigurable
}

/// Friendly provider display name, e.g. "OpenAI Codex" for "openai-codex".
/// Falls back to the raw provider id so unsupported providers stay visible.
fn provider_display_name(provider: &str) -> String {
    rho_providers::provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.display_name)
        .unwrap_or(provider)
        .to_string()
}

fn statusline_lines(
    state: &StatusLineState,
    width: usize,
    goal: Option<&GoalStatus>,
) -> Vec<Line<'static>> {
    let goal = goal.map(|goal| {
        let state = if goal.blocked { "blocked" } else { "active" };
        [
            format!(
                "goal: {state} • {} turn{} • {}",
                goal.turns,
                if goal.turns == 1 { "" } else { "s" },
                super::goal::format_elapsed(goal.elapsed)
            ),
            format!("goal: {state}"),
            state.into(),
        ]
    });
    let cwd_path = compact_cwd(&state.cwd);
    let cwd_branch = state.branch.as_deref();
    let top_left = format_cwd_left(&cwd_path, cwd_branch);
    let top_right = goal
        .as_ref()
        .map(|candidates| fit_right_status(&top_left, candidates, width))
        .unwrap_or_default();
    let (bottom_left, bottom_right) = bottom_status(state, width);
    vec![
        render_cwd_row(&cwd_path, cwd_branch, top_right, width),
        render_row(bottom_left, bottom_right, width),
    ]
}

fn bottom_status(state: &StatusLineState, width: usize) -> (String, String) {
    let permission = state.permission_mode.label().to_string();
    let model = if state.fast_mode_active {
        format!("{} (fast)", state.model)
    } else {
        state.model.clone()
    };

    let mut left = String::new();
    let context = format_context_summary(state);
    if !context.is_empty() && row_fits(&context, &permission, width) {
        left = context;
    }

    let right = fit_model_right(&left, &permission, &model, state, width);

    if let Some(cost) = status_cost(state) {
        append_left_if_fits(&mut left, &right, width, cost);
    }
    if let Some(rate) = state.average_output_rate {
        append_left_if_fits(&mut left, &right, width, format!("{rate} tok/s avg"));
    }
    (left, right)
}

/// Degrading right side: `permission · provider · model · reasoning`, dropping
/// reasoning, then the provider, then the model as space runs out. Reuses
/// [`fit_right_status`] so the provider never hides the model on its own.
fn fit_model_right(
    left: &str,
    permission: &str,
    model: &str,
    state: &StatusLineState,
    width: usize,
) -> String {
    let provider = provider_display_name(&state.provider);
    let full = format!("{permission} · {}", model_segment(&provider, model));

    let mut candidates = Vec::with_capacity(4);
    if state.reasoning_configurable {
        candidates.push(format!("{full} · {}", state.reasoning));
    }
    candidates.push(full);
    if !provider.is_empty() {
        candidates.push(format!("{permission} · {model}"));
    }
    candidates.push(permission.to_string());
    fit_right_status(left, &candidates, width)
}

fn model_segment(provider: &str, model: &str) -> String {
    if provider.is_empty() {
        model.to_string()
    } else {
        format!("{provider} · {model}")
    }
}

fn append_left_if_fits(left: &mut String, right: &str, width: usize, segment: String) {
    let appended = if left.is_empty() {
        segment
    } else {
        format!("{left} · {segment}")
    };
    if row_fits(&appended, right, width) {
        *left = appended;
    }
}

fn row_fits(left: &str, right: &str, width: usize) -> bool {
    let gap = usize::from(!left.is_empty() && !right.is_empty());
    display_width(left) + display_width(right) + gap <= width
}

fn format_context_summary(state: &StatusLineState) -> String {
    let Some(context) = state.context_usage.as_ref() else {
        return String::new();
    };
    let Some(window) = context
        .context_window
        .or_else(|| {
            state
                .model_metadata
                .as_ref()
                .and_then(ModelMetadata::display_context_window)
        })
        .filter(|window| *window > 0)
    else {
        return String::new();
    };
    let Some(tokens) = context.tokens else {
        return match context.source {
            ContextUsageSource::UnknownAfterCompaction => "?".into(),
            ContextUsageSource::Estimated | ContextUsageSource::ProviderReported => String::new(),
        };
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    format!("{} ({percent:.1}%)", format_token_count(tokens))
}

fn status_cost(state: &StatusLineState) -> Option<String> {
    let main_cost_micros = state
        .usage
        .as_ref()
        .and_then(|usage| resolved_usage_cost_usd_micros(usage, state.model_metadata.as_ref()));
    session_total_cost_usd_micros(main_cost_micros, state.subagent_total_cost_usd_micros)
        .map(format_usd)
}

fn fit_right_status(left: &str, candidates: &[String], width: usize) -> String {
    let full = &candidates[0];
    if display_width(left) + display_width(full) < width {
        return full.clone();
    }

    let separator_width = usize::from(!left.is_empty());
    let available = width
        .saturating_sub(display_width(left) + separator_width)
        .max(width.saturating_div(2))
        .max(1);
    candidates
        .iter()
        .find(|candidate| display_width(candidate) <= available)
        .cloned()
        .unwrap_or_else(|| {
            truncate_one_line(candidates.last().expect("status has a value"), available)
        })
}

fn render_row(left: String, right: String, width: usize) -> Line<'static> {
    match row_side_fit(display_width(&left), &right, width) {
        None => status_row_line(left, right, width),
        Some((left_budget, right)) => {
            status_row_line(truncate_one_line(&left, left_budget), right, width)
        }
    }
}

fn render_cwd_row(path: &str, branch: Option<&str>, right: String, width: usize) -> Line<'static> {
    let left = format_cwd_left(path, branch);
    match row_side_fit(display_width(&left), &right, width) {
        None => status_row_line(left, right, width),
        Some((left_budget, right)) => {
            status_row_line(fit_cwd(path, branch, left_budget), right, width)
        }
    }
}

/// Shared left/right budget math for a status row.
///
/// Returns `None` when both sides already fit. Otherwise right is head-truncated
/// first, then the caller fits left into the remaining budget.
fn row_side_fit(left_width: usize, right: &str, width: usize) -> Option<(usize, String)> {
    if right.is_empty() {
        return Some((width, String::new()));
    }

    let right_width = display_width(right);
    if left_width + right_width + usize::from(left_width > 0) <= width {
        return None;
    }

    let right_budget = right_width.min(width.saturating_div(2).max(1));
    let right = truncate_one_line(right, right_budget);
    let right_width = display_width(&right);
    let left_budget = width.saturating_sub(right_width + 1).max(1);
    Some((left_budget, right))
}

fn status_row_line(left: String, right: String, width: usize) -> Line<'static> {
    let style = Theme::dim();
    if right.is_empty() {
        return Line::from(Span::styled(left, style));
    }
    let gap = " ".repeat(width.saturating_sub(display_width(&left) + display_width(&right)));
    Line::from(Span::styled(format!("{left}{gap}{right}"), style))
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
