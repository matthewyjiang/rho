use std::{
    path::{Path, PathBuf},
    time::Duration,
};

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
    let top_left = state.left_top();
    let top_right = goal
        .as_ref()
        .map(|candidates| fit_right_status(&top_left, candidates, width))
        .unwrap_or_default();
    let (bottom_left, bottom_right) = bottom_status(state, width);
    vec![
        render_cwd_row(top_left, top_right, width),
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
    render_row_with_left_fit(left, right, width, truncate_one_line)
}

fn render_cwd_row(left: String, right: String, width: usize) -> Line<'static> {
    render_row_with_left_fit(left, right, width, fit_status_cwd_left)
}

fn render_row_with_left_fit(
    left: String,
    right: String,
    width: usize,
    fit_left: fn(&str, usize) -> String,
) -> Line<'static> {
    let style = Theme::dim();
    if right.is_empty() {
        return Line::from(Span::styled(fit_left(&left, width), style));
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
    let left = fit_left(&left, width.saturating_sub(right_width + 1).max(1));
    let left_width = display_width(&left);
    let gap = " ".repeat(width.saturating_sub(left_width + right_width));
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

/// Fit a status-line cwd (optional ` (branch)` suffix) into `width`.
///
/// Prefers keeping the trailing path segments that identify the workspace, using a
/// middle ellipsis for dropped leading segments: `~/work/…/api-gateway`.
fn fit_status_cwd_left(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }

    let (path, branch_suffix) = split_status_cwd_branch(text);
    if branch_suffix.is_empty() {
        return shorten_path_display(path, width);
    }

    let branch_width = display_width(branch_suffix);
    if branch_width >= width {
        // Branch alone fills the row; fall back to ordinary head truncation.
        return truncate_one_line(text, width);
    }

    let shortened = shorten_path_display(path, width - branch_width);
    format!("{shortened}{branch_suffix}")
}

fn split_status_cwd_branch(text: &str) -> (&str, &str) {
    if let Some(open) = text.rfind(" (") {
        if open > 0 && text.ends_with(')') {
            return (&text[..open], &text[open..]);
        }
    }
    (text, "")
}

fn shorten_path_display(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(path) <= width {
        return path.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }

    let sep = path_display_separator(path);
    let (prefix, rest) = split_path_display_prefix(path, sep);
    let segments: Vec<&str> = rest
        .split(sep)
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() <= 1 {
        // No interior segments to drop; keep the identifying end of the name.
        return truncate_keep_end(path, width);
    }

    // Prefer the longest trailing-segment form that still fits.
    let mut best: Option<String> = None;
    for keep in 1..segments.len() {
        let tail = segments[segments.len() - keep..].join(&sep.to_string());
        let candidate = if prefix.is_empty() {
            format!("…{sep}{tail}")
        } else {
            // `~/…/tail` or `/…/tail`
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

    // Even the shortest rooted form failed. Try dropping the root prefix:
    // `…/last` instead of `~/…/last`.
    let last = segments[segments.len() - 1];
    let minimal = format!("…{sep}{last}");
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

/// Truncate from the front, keeping the end of `text` with a leading ellipsis.
fn truncate_keep_end(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }

    let target = width - 1;
    let mut start = text.len();
    let mut used = 0usize;
    for (index, ch) in text.char_indices().rev() {
        let ch_width = super::render::char_display_width(ch);
        if used + ch_width > target {
            break;
        }
        used += ch_width;
        start = index;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
