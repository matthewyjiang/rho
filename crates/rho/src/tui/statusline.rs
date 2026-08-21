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
pub(super) mod path;
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
    /// Non-main session cost (subagents + advisor) folded into one total.
    extra_cost_usd_micros: u64,
    average_generation_rate: Option<u64>,
    /// The active provider resolved to usable credentials. When false the row
    /// names the gap instead of a model the session cannot reach.
    signed_in: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StatusLineCache {
    width: usize,
    goal: Option<GoalStatus>,
    theme_generation: u64,
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
            extra_cost_usd_micros: 0,
            average_generation_rate: None,
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
            extra_cost_usd_micros: 0,
            average_generation_rate: None,
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
        extra_cost_usd_micros: u64,
    ) {
        if self.state.usage.as_ref() != usage
            || self.state.context_usage.as_ref() != context_usage
            || self.state.extra_cost_usd_micros != extra_cost_usd_micros
        {
            self.state.usage = usage.cloned();
            self.state.context_usage = context_usage.cloned();
            self.state.extra_cost_usd_micros = extra_cost_usd_micros;
            self.invalidate();
        }
    }

    pub(super) fn update_signed_in(&mut self, signed_in: bool) {
        if self.state.signed_in != signed_in {
            self.state.signed_in = signed_in;
            self.invalidate();
        }
    }

    pub(super) fn update_average_generation_rate(&mut self, average_generation_rate: Option<u64>) {
        if self.state.average_generation_rate != average_generation_rate {
            self.state.average_generation_rate = average_generation_rate;
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
        let theme_generation = Theme::generation();
        if self.cache.lines.is_empty()
            || self.cache.width != width
            || self.cache.goal != goal
            || self.cache.theme_generation != theme_generation
        {
            let lines = statusline_lines(&self.state, width, goal.as_ref());
            self.cache.width = width;
            self.cache.goal = goal;
            self.cache.theme_generation = theme_generation;
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

// Drop ranks for the bottom row. Lower values drop first when width is scarce.
const RANK_REASONING: u8 = 1;
const RANK_RATE: u8 = 2;
const RANK_PROVIDER: u8 = 3;
const RANK_COST: u8 = 4;
const RANK_CONTEXT: u8 = 5;
const RANK_MODEL: u8 = 6;
const RANK_LOGIN_HINT: u8 = 6;
const RANK_PERMISSION: u8 = 7;
/// Signed-out copy outranks permission so the row still names the fix.
const RANK_SIGNED_OUT: u8 = 8;

/// Identity keys used by pack tests and paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKey {
    Context,
    Cost,
    Rate,
    Permission,
    Provider,
    Model,
    Reasoning,
    SignedOut,
    LoginHint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// One ranked field on the bottom status row.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusField {
    key: FieldKey,
    side: Side,
    /// Lower drops first.
    rank: u8,
    /// Paint order within a side (lower = further left).
    order: u8,
    text: String,
    style: Style,
}

fn field(
    key: FieldKey,
    side: Side,
    rank: u8,
    order: u8,
    text: impl Into<String>,
    style: Style,
) -> StatusField {
    StatusField {
        key,
        side,
        rank,
        order,
        text: text.into(),
        style,
    }
}

/// Permission mode style. Bypass skips every check, so it must not look ambient.
fn permission_style(mode: PermissionMode) -> Style {
    match mode {
        PermissionMode::Bypass => Theme::warning(),
        PermissionMode::Auto
        | PermissionMode::AllowEdits
        | PermissionMode::Plan
        | PermissionMode::Supervised => Theme::dim(),
    }
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
    let (bottom_left, bottom_right) = pack_bottom_status(state, width);
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
/// 2. generation rate
/// 3. provider label
/// 4. session cost
/// 5. context usage
/// 6. model id
/// 7. permission mode (kept last)
///
/// Severity is independent of drop rank: high context fill and Bypass permission
/// use warning/error styles so they stay visible while they remain on screen.
fn pack_bottom_status(
    state: &StatusLineState,
    width: usize,
) -> (Vec<StatusField>, Vec<StatusField>) {
    let mut fields = bottom_fields(state);
    drop_to_width(&mut fields, width);
    split_sides(fields)
}

fn bottom_fields(state: &StatusLineState) -> Vec<StatusField> {
    let mut fields = Vec::with_capacity(8);

    if let Some((text, style)) = format_context_summary(state) {
        fields.push(field(
            FieldKey::Context,
            Side::Left,
            RANK_CONTEXT,
            0,
            text,
            style,
        ));
    }
    if let Some(cost) = status_cost(state) {
        fields.push(field(
            FieldKey::Cost,
            Side::Left,
            RANK_COST,
            1,
            cost,
            Theme::dim(),
        ));
    }
    if let Some(rate) = state.average_generation_rate {
        fields.push(field(
            FieldKey::Rate,
            Side::Left,
            RANK_RATE,
            2,
            format!("{rate} tok/s"),
            Theme::dim(),
        ));
    }

    fields.push(field(
        FieldKey::Permission,
        Side::Right,
        RANK_PERMISSION,
        0,
        state.permission_mode.label(),
        permission_style(state.permission_mode),
    ));

    if !state.signed_in {
        // Naming the configured model would promise a turn the session cannot
        // run, so the row points at the fix instead.
        fields.push(field(
            FieldKey::SignedOut,
            Side::Right,
            RANK_SIGNED_OUT,
            1,
            "not signed in",
            Theme::warning(),
        ));
        fields.push(field(
            FieldKey::LoginHint,
            Side::Right,
            RANK_LOGIN_HINT,
            2,
            "/login",
            Theme::dim(),
        ));
        return fields;
    }

    let provider = provider_display_name(&state.provider);
    if !provider.is_empty() {
        fields.push(field(
            FieldKey::Provider,
            Side::Right,
            RANK_PROVIDER,
            1,
            provider,
            Theme::dim(),
        ));
    }

    let model = if state.fast_mode_active {
        format!("{} (fast)", state.model)
    } else {
        state.model.clone()
    };
    if !model.is_empty() {
        fields.push(field(
            FieldKey::Model,
            Side::Right,
            RANK_MODEL,
            2,
            model,
            Theme::dim(),
        ));
    }

    if state.reasoning_configurable {
        fields.push(field(
            FieldKey::Reasoning,
            Side::Right,
            RANK_REASONING,
            3,
            state.reasoning.to_string(),
            Theme::reasoning_input_border(state.reasoning),
        ));
    }

    fields
}

/// Drop lowest-rank fields until the row fits. Rank is the hierarchy; sides only
/// affect paint placement.
fn drop_to_width(fields: &mut Vec<StatusField>, width: usize) {
    while fields.len() > 1 && fields_row_width(fields) > width {
        let drop_at = fields
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.rank
                    .cmp(&right.rank)
                    // Stable tie-break: drop the rightmost/lowest paint priority first.
                    .then(right.order.cmp(&left.order))
                    .then(right.side.order().cmp(&left.side.order()))
            })
            .map(|(index, _)| index)
            .expect("fields is non-empty");
        fields.remove(drop_at);
    }

    if fields_row_width(fields) > width {
        truncate_fields(fields, width);
    }
}

impl Side {
    fn order(self) -> u8 {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

fn split_sides(mut fields: Vec<StatusField>) -> (Vec<StatusField>, Vec<StatusField>) {
    fields.sort_by(|left, right| {
        left.side
            .order()
            .cmp(&right.side.order())
            .then(left.order.cmp(&right.order))
    });
    let right_start = fields
        .iter()
        .position(|field| field.side == Side::Right)
        .unwrap_or(fields.len());
    let right = fields.split_off(right_start);
    (fields, right)
}

fn fields_row_width(fields: &[StatusField]) -> usize {
    let left_width = side_width(fields.iter().filter(|field| field.side == Side::Left));
    let right_width = side_width(fields.iter().filter(|field| field.side == Side::Right));
    let gap = usize::from(left_width > 0 && right_width > 0);
    left_width + right_width + gap
}

fn side_width<'a>(fields: impl IntoIterator<Item = &'a StatusField>) -> usize {
    let mut count = 0usize;
    let mut text_width = 0usize;
    for field in fields {
        text_width += display_width(&field.text);
        count += 1;
    }
    if count == 0 {
        return 0;
    }
    text_width + display_width(FIELD_SEP) * (count - 1)
}

fn truncate_fields(fields: &mut [StatusField], width: usize) {
    if fields.is_empty() || width == 0 {
        for field in fields.iter_mut() {
            field.text.clear();
        }
        return;
    }

    // After rank drops, at most a couple fields remain. Shrink text from the
    // lowest-rank survivor until the row fits.
    while fields_row_width(fields) > width {
        let index = fields
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.rank
                    .cmp(&right.rank)
                    .then(right.order.cmp(&left.order))
            })
            .map(|(index, _)| index)
            .expect("fields is non-empty");
        let current = display_width(&fields[index].text);
        if current <= 1 {
            break;
        }
        let next = current.saturating_sub(1).max(1);
        fields[index].text = truncate_one_line(&fields[index].text, next);
    }
}

fn format_context_summary(state: &StatusLineState) -> Option<(String, Style)> {
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
            ContextUsageSource::UnknownAfterCompaction => Some(("?".into(), Theme::warning())),
            ContextUsageSource::Estimated | ContextUsageSource::ProviderReported => None,
        };
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    Some((
        format!("{} ({percent:.1}%)", format_token_count(tokens)),
        context_usage_style(percent),
    ))
}

fn status_cost(state: &StatusLineState) -> Option<String> {
    let main_cost_micros = state
        .usage
        .as_ref()
        .and_then(|usage| resolved_usage_cost_usd_micros(usage, state.model_metadata.as_ref()));
    session_total_cost_usd_micros(main_cost_micros, state.extra_cost_usd_micros).map(format_usd)
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

fn render_status_row(
    left: Vec<StatusField>,
    right: Vec<StatusField>,
    width: usize,
) -> Line<'static> {
    status_fields_line(&left, &right, width)
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

fn status_fields_line(left: &[StatusField], right: &[StatusField], width: usize) -> Line<'static> {
    if right.is_empty() {
        return Line::from(fields_to_spans(left));
    }

    let left_width = side_width(left);
    let right_width = side_width(right);
    let gap = " ".repeat(width.saturating_sub(left_width + right_width));
    let mut spans = fields_to_spans(left);
    spans.push(Span::styled(gap, Theme::dim()));
    spans.extend(fields_to_spans(right));
    Line::from(spans)
}

fn fields_to_spans(fields: &[StatusField]) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(fields.len().saturating_mul(2).saturating_sub(1).max(1));
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(FIELD_SEP.to_string(), Theme::dim()));
        }
        spans.push(Span::styled(field.text.clone(), field.style));
    }
    spans
}

#[cfg(test)]
fn packed_keys(state: &StatusLineState, width: usize) -> Vec<FieldKey> {
    let (left, right) = pack_bottom_status(state, width);
    left.into_iter()
        .chain(right)
        .map(|field| field.key)
        .collect()
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
