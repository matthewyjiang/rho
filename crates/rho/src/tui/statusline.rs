use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use ratatui::text::{Line, Span};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
    RuntimeModelView,
};
use {
    crate::permission::PermissionMode,
    rho_providers::model::{
        ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage, ReasoningCapabilities,
    },
    rho_providers::reasoning::ReasoningLevel,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusLineState {
    cwd: PathBuf,
    branch: Option<String>,
    usage: Option<ModelUsage>,
    context_usage: Option<ContextUsage>,
    provider: String,
    model: String,
    reasoning: ReasoningLevel,
    reasoning_configurable: bool,
    permission_mode: PermissionMode,
    model_metadata: Option<ModelMetadata>,
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
            reasoning: ReasoningLevel::default(),
            reasoning_configurable: true,
            permission_mode: PermissionMode::default(),
            model_metadata: None,
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
            reasoning: info.reasoning,
            reasoning_configurable: reasoning_is_configurable(&info.provider, &info.model),
            permission_mode: info.permission_mode,
            model_metadata: None,
        }
    }

    fn left_top(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{} ({branch})", compact_cwd(&self.cwd)),
            None => compact_cwd(&self.cwd),
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
        if self.state.provider != info.provider
            || self.state.model != info.model
            || self.state.reasoning != info.reasoning
            || self.state.reasoning_configurable != reasoning_configurable
            || self.state.permission_mode != info.permission_mode
        {
            self.state.provider.clone_from(&info.provider);
            self.state.model.clone_from(&info.model);
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
    ) {
        if self.state.usage.as_ref() != usage || self.state.context_usage.as_ref() != context_usage
        {
            self.state.usage = usage.cloned();
            self.state.context_usage = context_usage.cloned();
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
    let top_left = state.left_top();
    let top_right = goal
        .as_ref()
        .map(|candidates| fit_right_status(&top_left, candidates, width))
        .unwrap_or_default();
    let bottom_left = format_context_summary(state);
    let bottom_right = compact_runtime_status(state, &bottom_left, width);
    let bottom_left = add_cost_if_it_fits(state, bottom_left, &bottom_right, width);
    vec![
        render_row(top_left, top_right, width),
        render_row(bottom_left, bottom_right, width),
    ]
}

fn compact_runtime_status(state: &StatusLineState, left: &str, width: usize) -> String {
    let mut status = state.permission_mode.label().to_string();
    let separator = " · ";
    let available = width.saturating_sub(display_width(left) + usize::from(!left.is_empty()));
    if available <= display_width(&status) {
        return status;
    }

    let model_budget = available.saturating_sub(display_width(&status) + display_width(separator));
    if model_budget >= 6 {
        status.push_str(separator);
        status.push_str(&truncate_one_line(&state.model, model_budget));
    }

    if state.reasoning_configurable {
        let with_reasoning = format!("{status}{separator}{}", state.reasoning);
        if display_width(&with_reasoning) <= available {
            status = with_reasoning;
        }
    }
    status
}

fn add_cost_if_it_fits(
    state: &StatusLineState,
    mut left: String,
    right: &str,
    width: usize,
) -> String {
    let Some(cost) = status_cost(state) else {
        return left;
    };
    let candidate = if left.is_empty() {
        cost
    } else {
        format!("{left} · {cost}")
    };
    let gap = usize::from(!candidate.is_empty() && !right.is_empty());
    if display_width(&candidate) + display_width(right) + gap <= width {
        left = candidate;
    }
    left
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
    let marker = match context.source {
        ContextUsageSource::Estimated => "~",
        ContextUsageSource::ProviderReported => "",
        ContextUsageSource::UnknownAfterCompaction => "?",
    };
    let Some(tokens) = context.tokens else {
        return format!("{marker}ctx");
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    format!("{marker}{percent:.1}% ctx")
}

fn status_cost(state: &StatusLineState) -> Option<String> {
    let usage = state.usage.as_ref()?;
    usage
        .cost_usd_micros
        .or_else(|| estimated_cost_usd_micros(usage, state.model_metadata.as_ref()))
        .map(|cost| format!("${}", format_usd(cost)))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BillingInfo {
    Metered,
    Subscription,
}

impl BillingInfo {
    pub(super) fn from_provider_auth(provider: &str, auth: &str) -> Self {
        if provider == "openai-codex" || auth == "codex" || auth == "xai-oauth" {
            Self::Subscription
        } else {
            Self::Metered
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Metered => "metered API",
            Self::Subscription => "subscription",
        }
    }
}

pub(super) fn cache_hit_percent(usage: Option<&ModelUsage>) -> Option<f64> {
    let usage = usage?;
    let cache_read = usage.cache_read_tokens?;
    let prompt_tokens = usage
        .input_tokens
        .unwrap_or_default()
        .saturating_add(cache_read);
    (prompt_tokens > 0).then(|| cache_read as f64 * 100.0 / prompt_tokens as f64)
}

pub(super) fn estimated_cost_usd_micros(
    usage: &ModelUsage,
    metadata: Option<&ModelMetadata>,
) -> Option<u64> {
    let metadata = metadata?;
    let input = usage.input_tokens.unwrap_or_default();
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let total_input = usage.total_input_tokens().unwrap_or_default();
    let cost = metadata.cost_for_input_tokens(total_input)?;
    let mut micros = 0u128;
    micros += cost_component(input, cost.input_micros_per_m);
    micros += cost_component(
        usage.output_tokens.unwrap_or_default(),
        cost.output_micros_per_m,
    );
    micros += cost_component(cache_read, cost.cache_read_micros_per_m);
    micros += cost_component(
        usage.cache_write_tokens.unwrap_or_default(),
        cost.cache_write_micros_per_m,
    );
    (micros > 0).then_some(micros.min(u64::MAX as u128) as u64)
}

fn cost_component(tokens: u64, micros_per_million: Option<u64>) -> u128 {
    tokens as u128 * micros_per_million.unwrap_or_default() as u128 / 1_000_000
}

fn format_usd(micros: u64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    if dollars >= 100.0 {
        format!("{dollars:.0}")
    } else if dollars >= 10.0 {
        format!("{dollars:.2}")
    } else {
        format!("{dollars:.3}")
    }
}

fn render_row(left: String, right: String, width: usize) -> Line<'static> {
    let style = Theme::dim();
    if right.is_empty() {
        return Line::from(Span::styled(truncate_one_line(&left, width), style));
    }

    let left_width = display_width(&left);
    let right_width = display_width(&right);
    if left_width + right_width + usize::from(!left.is_empty()) <= width {
        let gap = " ".repeat(width - left_width - right_width);
        return Line::from(Span::styled(format!("{left}{gap}{right}"), style));
    }

    let right_budget = right_width.min(width.saturating_div(2).max(1));
    let right = truncate_one_line(&right, right_budget);
    let right_width = display_width(&right);
    let left = truncate_one_line(&left, width.saturating_sub(right_width + 1).max(1));
    let left_width = display_width(&left);
    let gap = " ".repeat(width.saturating_sub(left_width + right_width));
    Line::from(Span::styled(format!("{left}{gap}{right}"), style))
}

pub(super) fn git_branch(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/")
        .map(ToString::to_string)
        .or_else(|| head.get(..7).map(ToString::to_string))
}

fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let contents = fs::read_to_string(&dot_git).ok()?;
            let path = contents.trim().strip_prefix("gitdir: ")?;
            let path = Path::new(path);
            return Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                dir.join(path)
            });
        }
    }
    None
}

fn compact_cwd(path: &Path) -> String {
    let Some(home) = crate::paths::home_dir() else {
        return path.display().to_string();
    };

    if let Ok(rest) = path.strip_prefix(home) {
        let rel = rest.display().to_string();
        if rel.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rel}")
        }
    } else {
        path.display().to_string()
    }
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
