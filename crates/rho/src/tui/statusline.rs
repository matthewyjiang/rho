use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use ratatui::text::{Line, Span};

use super::{
    render::{display_width, truncate_keep_end, truncate_one_line},
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
    subagent_total_cost_usd_micros: u64,
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
            subagent_total_cost_usd_micros: 0,
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
            subagent_total_cost_usd_micros: 0,
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
    let mut left = String::new();
    let mut right = state.permission_mode.label().to_string();

    let context = format_context_summary(state);
    if !context.is_empty() && row_fits(&context, &right, width) {
        left = context;
    }

    let with_model = format!("{right} · {}", state.model);
    if !row_fits(&left, &with_model, width) {
        return (left, right);
    }
    right = with_model;

    if state.reasoning_configurable {
        let with_reasoning = format!("{right} · {}", state.reasoning);
        if !row_fits(&left, &with_reasoning, width) {
            return (left, right);
        }
        right = with_reasoning;
    }

    let Some(cost) = status_cost(state) else {
        return (left, right);
    };
    let with_cost = if left.is_empty() {
        cost
    } else {
        format!("{left} · {cost}")
    };
    if row_fits(&with_cost, &right, width) {
        left = with_cost;
    }
    (left, right)
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

fn format_cwd_left(path: &str, branch: Option<&str>) -> String {
    match branch {
        Some(branch) => format!("{path} ({branch})"),
        None => path.to_string(),
    }
}

/// Fit cwd path + optional branch into `width`.
///
/// Basename visibility outranks the branch suffix. Degradation order:
/// 1. full `path (branch)`
/// 2. shortened path + branch, only while the full basename remains
/// 3. drop branch
/// 4. shortened path (may end-truncate a too-long final segment)
fn fit_cwd(path: &str, branch: Option<&str>, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let full = format_cwd_left(path, branch);
    if display_width(&full) <= width {
        return full;
    }

    if let Some(branch) = branch {
        let suffix = format!(" ({branch})");
        let suffix_width = display_width(&suffix);
        if suffix_width < width {
            let path_budget = width - suffix_width;
            if let Some(shortened) = shorten_path_keeping_basename(path, path_budget) {
                return format!("{shortened}{suffix}");
            }
        }
        // Branch is optional chrome; drop it before mangling the basename.
    }

    shorten_path_display(path, width)
}

fn shorten_path_keeping_basename(path: &str, width: usize) -> Option<String> {
    if display_width(path) <= width {
        return Some(path.to_string());
    }
    let shortened = shorten_path_display(path, width);
    retains_full_basename(path, &shortened).then_some(shortened)
}

fn retains_full_basename(path: &str, shortened: &str) -> bool {
    let base = path_basename(path);
    if base.is_empty() {
        return true;
    }
    if shortened == base {
        return true;
    }
    let Some(prefix) = shortened.strip_suffix(base) else {
        return false;
    };
    prefix.ends_with('/') || prefix.ends_with('\\')
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// Shorten a display path by dropping leading segments.
///
/// Keeps a root marker when it still fits (`~/…/api-gateway`, `/…/api-gateway`),
/// otherwise falls back to `…/api-gateway`, then end-truncates the last segment.
fn shorten_path_display(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(path) <= width {
        return path.to_string();
    }
    if width <= 1 {
        return truncate_keep_end(path, width);
    }

    let sep = path_display_separator(path);
    let (prefix, rest) = split_path_display_prefix(path, sep);
    let segments: Vec<&str> = rest
        .split(sep)
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() <= 1 {
        return truncate_keep_end(path, width);
    }

    // Prefer the longest trailing-segment form that still fits.
    let mut best: Option<String> = None;
    for keep in 1..segments.len() {
        let tail = segments[segments.len() - keep..].join(sep.to_string().as_str());
        let candidate = if prefix.is_empty() {
            format!("…{sep}{tail}")
        } else {
            format!("{prefix}…{sep}{tail}")
        };

        if display_width(&candidate) <= width {
            best = Some(candidate);
        } else if best.is_some() {
            // Further candidates only grow.
            break;
        }
    }

    if let Some(candidate) = best {
        return candidate;
    }

    // Even the shortest rooted form failed. Drop the root prefix:
    // `…/last` instead of `~/…/last`.
    let minimal = format!("…{sep}{}", segments[segments.len() - 1]);
    if display_width(&minimal) <= width {
        return minimal;
    }

    // Last segment itself is too long: keep its end so the name stays identifiable.
    truncate_keep_end(&minimal, width)
}

fn path_display_separator(path: &str) -> char {
    if path.contains('/') {
        '/'
    } else if path.contains('\\') {
        '\\'
    } else {
        '/'
    }
}

fn split_path_display_prefix(path: &str, sep: char) -> (&str, &str) {
    if let Some(rest) = path.strip_prefix("~/") {
        return ("~/", rest);
    }
    if let Some(rest) = path.strip_prefix(sep) {
        return (&path[..sep.len_utf8()], rest);
    }
    ("", path)
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
