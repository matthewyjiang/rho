use std::{path::PathBuf, time::Duration};

use ratatui::{
    style::Style,
    text::{Line, Span},
};

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
    /// The active provider resolved to usable credentials. When false the row
    /// names the gap instead of a model the session cannot reach.
    signed_in: bool,
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
            signed_in: true,
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
            signed_in: true,
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

    pub(super) fn update_signed_in(&mut self, signed_in: bool) {
        if self.state.signed_in != signed_in {
            self.state.signed_in = signed_in;
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

/// Separator between bottom-row status fields.
const FIELD_SEP: &str = " · ";

/// Context fill tripwires. Below warning the field stays ambient; warning and
/// critical escalate so a nearly full window cannot hide in dim chrome.
const CONTEXT_WARNING_PERCENT: f64 = 75.0;
const CONTEXT_CRITICAL_PERCENT: f64 = 90.0;

/// One painted field on the bottom status row.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusSegment {
    text: String,
    style: Style,
}

fn status_segment(text: impl Into<String>, style: Style) -> StatusSegment {
    StatusSegment {
        text: text.into(),
        style,
    }
}

fn segments_text(segments: &[StatusSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(FIELD_SEP)
}

fn segments_width(segments: &[StatusSegment]) -> usize {
    if segments.is_empty() {
        return 0;
    }
    let text_width = segments
        .iter()
        .map(|segment| display_width(&segment.text))
        .sum::<usize>();
    let sep_width = display_width(FIELD_SEP) * segments.len().saturating_sub(1);
    text_width + sep_width
}

fn segments_to_spans(segments: &[StatusSegment]) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(segments.len().saturating_mul(2).saturating_sub(1).max(1));
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(FIELD_SEP.to_string(), Theme::dim()));
        }
        spans.push(Span::styled(segment.text.clone(), segment.style));
    }
    spans
}

/// Permission mode style. Auto skips every check, so it must not look ambient.
fn permission_style(mode: PermissionMode) -> Style {
    match mode {
        PermissionMode::Auto => Theme::warning(),
        PermissionMode::Plan | PermissionMode::Supervised => Theme::dim(),
    }
}

fn permission_segment(mode: PermissionMode) -> StatusSegment {
    status_segment(mode.label(), permission_style(mode))
}

/// Context fill style. Escalates only when the window is meaningfully full.
fn context_usage_style(percent: f64) -> Style {
    if percent >= CONTEXT_CRITICAL_PERCENT {
        Theme::error()
    } else if percent >= CONTEXT_WARNING_PERCENT {
        Theme::warning()
    } else {
        Theme::dim()
    }
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
        render_status_row(bottom_left, bottom_right, width),
    ]
}

/// Bottom row layout with an explicit field hierarchy.
///
/// Sides:
/// - left metrics: `context · cost · rate`
/// - right identity: `permission · provider · model · reasoning`
///
/// Drop order when width is scarce (first dropped first):
/// 1. reasoning
/// 2. output rate
/// 3. provider label
/// 4. session cost
/// 5. context usage
/// 6. model id
/// 7. permission mode (kept last)
///
/// Severity is independent of drop rank: high context fill and Auto permission
/// use warning/error styles so they stay visible while they remain on screen.
fn bottom_status(
    state: &StatusLineState,
    width: usize,
) -> (Vec<StatusSegment>, Vec<StatusSegment>) {
    let model = if state.fast_mode_active {
        format!("{} (fast)", state.model)
    } else {
        state.model.clone()
    };

    let mut left = Vec::new();
    if let Some(context) = format_context_summary(state) {
        // Context is only kept when it still fits beside bare permission, the
        // last right-side field in the drop order.
        if row_fits(&context.text, state.permission_mode.label(), width) {
            left.push(context);
        }
    }

    let right = fit_model_right(segments_width(&left), &model, state, width);

    // Optional left metrics after right identity is fixed. Cost outranks rate.
    if let Some(cost) = status_cost(state) {
        append_left_if_fits(&mut left, &right, width, status_segment(cost, Theme::dim()));
    }
    if let Some(rate) = state.average_output_rate {
        append_left_if_fits(
            &mut left,
            &right,
            width,
            status_segment(format!("{rate} tok/s avg"), Theme::dim()),
        );
    }
    (left, right)
}

/// Right-side candidates in keep order. Each step drops the next field from the
/// ranked list: reasoning, then provider, then model, leaving permission.
fn fit_model_right(
    left_width: usize,
    model: &str,
    state: &StatusLineState,
    width: usize,
) -> Vec<StatusSegment> {
    if !state.signed_in {
        // Naming the configured model would promise a turn the session cannot
        // run, so the row points at the fix instead.
        let permission = permission_segment(state.permission_mode);
        let signed_out = status_segment("not signed in", Theme::warning());
        return fit_right_segments(
            left_width,
            &[
                vec![
                    permission.clone(),
                    signed_out.clone(),
                    status_segment("/login", Theme::dim()),
                ],
                vec![permission, signed_out.clone()],
                vec![signed_out],
            ],
            width,
        );
    }

    let permission = permission_segment(state.permission_mode);
    let provider = provider_display_name(&state.provider);
    let model_seg = status_segment(model, Theme::dim());

    let mut with_provider = vec![permission.clone()];
    if !provider.is_empty() {
        with_provider.push(status_segment(provider.clone(), Theme::dim()));
    }
    with_provider.push(model_seg.clone());

    let mut candidates = Vec::with_capacity(4);
    if state.reasoning_configurable {
        let mut with_reasoning = with_provider.clone();
        with_reasoning.push(status_segment(state.reasoning.to_string(), Theme::dim()));
        candidates.push(with_reasoning);
    }
    candidates.push(with_provider);
    if !provider.is_empty() {
        candidates.push(vec![permission.clone(), model_seg]);
    }
    candidates.push(vec![permission]);
    fit_right_segments(left_width, &candidates, width)
}

#[cfg(test)]
fn model_segment(provider: &str, model: &str) -> String {
    if provider.is_empty() {
        model.to_string()
    } else {
        format!("{provider}{FIELD_SEP}{model}")
    }
}

fn append_left_if_fits(
    left: &mut Vec<StatusSegment>,
    right: &[StatusSegment],
    width: usize,
    segment: StatusSegment,
) {
    let mut trial = left.clone();
    trial.push(segment);
    if row_fits(&segments_text(&trial), &segments_text(right), width) {
        *left = trial;
    }
}

fn row_fits(left: &str, right: &str, width: usize) -> bool {
    let gap = usize::from(!left.is_empty() && !right.is_empty());
    display_width(left) + display_width(right) + gap <= width
}

fn format_context_summary(state: &StatusLineState) -> Option<StatusSegment> {
    let context = state.context_usage.as_ref()?;
    let window = context
        .context_window
        .or_else(|| {
            state
                .model_metadata
                .as_ref()
                .and_then(ModelMetadata::display_context_window)
        })
        .filter(|window| *window > 0)?;
    let Some(tokens) = context.tokens else {
        return match context.source {
            // Unknown after compaction is a real gap, not ambient chrome.
            ContextUsageSource::UnknownAfterCompaction => {
                Some(status_segment("?", Theme::warning()))
            }
            ContextUsageSource::Estimated | ContextUsageSource::ProviderReported => None,
        };
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    Some(status_segment(
        format!("{} ({percent:.1}%)", format_token_count(tokens)),
        context_usage_style(percent),
    ))
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

fn fit_right_segments(
    left_width: usize,
    candidates: &[Vec<StatusSegment>],
    width: usize,
) -> Vec<StatusSegment> {
    let full = &candidates[0];
    let full_width = segments_width(full);
    if left_width + full_width < width {
        return full.clone();
    }

    let separator_width = usize::from(left_width > 0);
    let available = width
        .saturating_sub(left_width + separator_width)
        .max(width.saturating_div(2))
        .max(1);
    candidates
        .iter()
        .find(|candidate| segments_width(candidate) <= available)
        .cloned()
        .unwrap_or_else(|| {
            truncate_segments(candidates.last().expect("status has a value"), available)
        })
}

fn truncate_segments(segments: &[StatusSegment], available: usize) -> Vec<StatusSegment> {
    if segments.is_empty() || available == 0 {
        return Vec::new();
    }

    let mut out = segments.to_vec();
    while out.len() > 1 && segments_width(&out) > available {
        out.pop();
    }

    let width = segments_width(&out);
    if width <= available {
        return out;
    }

    if let Some(last) = out.last_mut() {
        let prefix_width = width.saturating_sub(display_width(&last.text));
        let budget = available.saturating_sub(prefix_width).max(1);
        last.text = truncate_one_line(&last.text, budget);
    }
    out
}

fn render_status_row(
    left: Vec<StatusSegment>,
    right: Vec<StatusSegment>,
    width: usize,
) -> Line<'static> {
    let left_width = segments_width(&left);
    let right_width = segments_width(&right);
    let gap = usize::from(left_width > 0 && right_width > 0);
    if left_width + right_width + gap <= width {
        return status_segments_line(left, right, width);
    }

    // Safety net: identity on the right keeps half the row before left shrinks.
    let right_budget = right_width.min(width.saturating_div(2).max(1));
    let right = truncate_segments(&right, right_budget);
    let right_width = segments_width(&right);
    let left_budget = width.saturating_sub(right_width + 1).max(1);
    let left = truncate_segments(&left, left_budget);
    status_segments_line(left, right, width)
}

#[cfg(test)]
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

fn status_segments_line(
    left: Vec<StatusSegment>,
    right: Vec<StatusSegment>,
    width: usize,
) -> Line<'static> {
    if right.is_empty() {
        return Line::from(segments_to_spans(&left));
    }

    let left_width = segments_width(&left);
    let right_width = segments_width(&right);
    let gap = " ".repeat(width.saturating_sub(left_width + right_width));
    let mut spans = segments_to_spans(&left);
    spans.push(Span::styled(gap, Theme::dim()));
    spans.extend(segments_to_spans(&right));
    Line::from(spans)
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
