use std::path::PathBuf;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
    usage_cost::{
        context_fill_percent, format_token_count, format_usd, resolved_context_window,
        resolved_usage_cost_usd_micros, session_total_cost_usd_micros,
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
use path::{compact_cwd, fit_cwd_row};

#[cfg(test)]
use path::{fit_cwd, shorten_path_display};

#[path = "statusline_fields.rs"]
mod field_row;
pub(super) use field_row::StatusClick;
use field_row::{click_action, status_fields_line, StatusFieldHit};

/// Optional cwd-row suffix. Tone is resolved at paint so a theme switch
/// recolors without waiting for another probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CwdExtra {
    text: String,
    tone: CwdExtraTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CwdExtraTone {
    Success,
    Error,
    Dim,
}

impl CwdExtra {
    pub(super) fn new(text: impl Into<String>, tone: CwdExtraTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }

    fn style(&self) -> Style {
        match self.tone {
            CwdExtraTone::Success => Theme::success(),
            CwdExtraTone::Error => Theme::error(),
            CwdExtraTone::Dim => Theme::dim(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusLineState {
    cwd: PathBuf,
    branch: Option<String>,
    cwd_extra: Option<CwdExtra>,
    usage: Option<ModelUsage>,
    context_usage: Option<ContextUsage>,
    provider: String,
    model: String,
    /// The model's display name, shown in place of its id when known.
    model_name: Option<String>,
    fast_mode_active: bool,
    zen_mode: bool,
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
    not_saved: bool,
    /// Click target under the pointer; its fields paint with a hover lift.
    hovered: Option<StatusClick>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StatusLineCache {
    width: usize,
    theme_generation: u64,
    lines: Vec<Line<'static>>,
    /// Clickable field spans painted on [`FIELDS_ROW`] by the same render.
    hits: Vec<StatusFieldHit>,
    #[cfg(test)]
    render_count: usize,
}

/// Index of the ranked-field row in [`StatusLine::lines`]; the cwd row sits above it.
pub(super) const FIELDS_ROW: usize = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct StatusLine {
    state: StatusLineState,
    cache: StatusLineCache,
}

impl Default for StatusLineState {
    fn default() -> Self {
        Self {
            cwd: PathBuf::new(),
            branch: None,
            cwd_extra: None,
            usage: None,
            context_usage: None,
            provider: String::new(),
            model: String::new(),
            model_name: None,
            fast_mode_active: false,
            zen_mode: false,
            reasoning: ReasoningLevel::default(),
            reasoning_configurable: true,
            permission_mode: PermissionMode::default(),
            model_metadata: None,
            extra_cost_usd_micros: 0,
            average_generation_rate: None,
            signed_in: true,
            not_saved: false,
            hovered: None,
        }
    }
}

impl StatusLineState {
    fn from_tui(info: &RuntimeModelView) -> Self {
        Self {
            cwd: info.cwd.clone(),
            branch: git_branch(&info.cwd),
            cwd_extra: None,
            usage: None,
            context_usage: None,
            provider: info.provider.clone(),
            model: info.model.clone(),
            model_name: None,
            fast_mode_active: info.fast_mode_active(),
            zen_mode: info.zen_mode,
            reasoning: info.reasoning,
            reasoning_configurable: reasoning_is_configurable(&info.provider, &info.model),
            permission_mode: info.permission_mode,
            model_metadata: None,
            extra_cost_usd_micros: 0,
            average_generation_rate: None,
            signed_in: true,
            not_saved: false,
            hovered: None,
        }
    }
}

/// Display controls supplied by the owner of the session's settings.
pub(super) struct ModelControls {
    pub(super) fast_mode_active: bool,
    pub(super) reasoning_configurable: bool,
}

impl StatusLine {
    pub(super) fn update_not_saved(&mut self, not_saved: bool) {
        if self.state.not_saved != not_saved {
            self.state.not_saved = not_saved;
            self.invalidate();
        }
    }

    pub(super) fn new(info: &RuntimeModelView) -> Self {
        Self {
            state: StatusLineState::from_tui(info),
            cache: StatusLineCache::default(),
        }
    }

    pub(super) fn branch(&self) -> Option<&str> {
        self.state.branch.as_deref()
    }

    /// Names `cwd`, with its branch, in place of the directory shown.
    pub(super) fn move_to(&mut self, cwd: PathBuf) {
        self.state.branch = git_branch(&cwd);
        self.state.cwd = cwd;
        self.state.cwd_extra = None;
        self.invalidate();
    }

    pub(super) fn refresh_git_branch(&mut self) -> bool {
        let branch = git_branch(&self.state.cwd);
        if self.state.branch == branch {
            return false;
        }
        self.state.branch = branch;
        self.state.cwd_extra = None;
        self.invalidate();
        true
    }

    pub(super) fn update_cwd_extra(&mut self, cwd_extra: Option<CwdExtra>) {
        if self.state.cwd_extra != cwd_extra {
            self.state.cwd_extra = cwd_extra;
            self.invalidate();
        }
    }

    /// Updates the model chrome from the model view. Reasoning
    /// capability is read from the model catalog only when the model view
    /// changed, not on every frame.
    pub(super) fn update_model(&mut self, info: &RuntimeModelView) {
        let fast_mode_active = info.fast_mode_active();
        if self.model_changed(info, fast_mode_active) {
            let configurable = reasoning_is_configurable(&info.provider, &info.model);
            self.set_model(info, fast_mode_active, configurable);
        }
    }

    /// Updates the model chrome from controls the session's settings owner,
    /// a daemon, supplies.
    pub(super) fn update_model_controls(
        &mut self,
        info: &RuntimeModelView,
        controls: ModelControls,
    ) {
        if self.model_changed(info, controls.fast_mode_active)
            || self.state.reasoning_configurable != controls.reasoning_configurable
        {
            self.set_model(
                info,
                controls.fast_mode_active,
                controls.reasoning_configurable,
            );
        }
    }

    fn model_changed(&self, info: &RuntimeModelView, fast_mode_active: bool) -> bool {
        self.state.provider != info.provider
            || self.state.model != info.model
            || self.state.fast_mode_active != fast_mode_active
            || self.state.zen_mode != info.zen_mode
            || self.state.reasoning != info.reasoning
            || self.state.permission_mode != info.permission_mode
    }

    fn set_model(
        &mut self,
        info: &RuntimeModelView,
        fast_mode_active: bool,
        reasoning_configurable: bool,
    ) {
        self.state.provider.clone_from(&info.provider);
        self.state.model.clone_from(&info.model);
        self.state.fast_mode_active = fast_mode_active;
        self.state.zen_mode = info.zen_mode;
        self.state.reasoning = info.reasoning;
        self.state.reasoning_configurable = reasoning_configurable;
        self.state.permission_mode = info.permission_mode;
        self.invalidate();
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

    /// Sets the model's display name; `None` shows the model id.
    pub(super) fn update_model_name(&mut self, model_name: Option<String>) {
        if self.state.model_name != model_name {
            self.state.model_name = model_name;
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
        if self.state.model_metadata.as_ref() != model_metadata {
            self.state.model_metadata = model_metadata.cloned();
            self.refresh_reasoning_capabilities();
            self.invalidate();
        }
    }

    /// Catalog completion can change capabilities without changing model identity.
    pub(super) fn refresh_reasoning_capabilities(&mut self) {
        let configurable = reasoning_is_configurable(&self.state.provider, &self.state.model);
        if self.state.reasoning_configurable != configurable {
            self.state.reasoning_configurable = configurable;
            self.invalidate();
        }
    }

    pub(super) fn lines(&mut self, width: usize) -> &[Line<'static>] {
        let theme_generation = Theme::generation();
        if self.cache.lines.is_empty()
            || self.cache.width != width
            || self.cache.theme_generation != theme_generation
        {
            let (lines, hits) = statusline_lines(&self.state, width);
            self.cache.width = width;
            self.cache.theme_generation = theme_generation;
            self.cache.lines = lines;
            self.cache.hits = hits;
            #[cfg(test)]
            {
                self.cache.render_count += 1;
            }
        }
        &self.cache.lines
    }

    /// Clickable field painted at row-relative `column` of [`FIELDS_ROW`] in
    /// the last [`Self::lines`] render. Callers render first so the spans
    /// match the current width.
    pub(super) fn hit_at(&self, column: usize) -> Option<&StatusFieldHit> {
        self.cache
            .hits
            .iter()
            .find(|hit| hit.columns.contains(&column))
    }

    /// Hover the click target painted at row-relative `column` of
    /// [`FIELDS_ROW`], or clear hover with `None`. A change repaints the row.
    pub(super) fn set_hovered_column(&mut self, column: Option<usize>) {
        let hovered = column
            .and_then(|column| self.hit_at(column))
            .map(|hit| hit.action);
        if self.state.hovered != hovered {
            self.state.hovered = hovered;
            self.invalidate();
        }
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
        self.cache.hits.clear();
    }
}

pub(super) fn reasoning_is_configurable(provider: &str, model: &str) -> bool {
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
// Decade gaps leave room for severity promotion (context urgent at 65).
const RANK_REASONING: u8 = 10;
const RANK_RATE: u8 = 20;
const RANK_ZEN: u8 = 30;
const RANK_PROVIDER: u8 = 40;
const RANK_COST: u8 = 50;
/// Ambient context fill; drops before model until warning/critical promotion.
const RANK_CONTEXT: u8 = 55;
const RANK_MODEL: u8 = 60;
const RANK_LOGIN_HINT: u8 = 60;
/// Warning/critical context outranks model but stays below permission.
const RANK_CONTEXT_URGENT: u8 = 65;
/// Unsaved-session warning outranks model identity but never hides permission mode.
const RANK_NOT_SAVED: u8 = RANK_CONTEXT_URGENT;
const RANK_PERMISSION: u8 = 70;
/// Signed-out copy outranks permission so the row still names the fix.
const RANK_SIGNED_OUT: u8 = 80;

/// Identity keys used by pack tests and paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKey {
    Context,
    Cost,
    Rate,
    Permission,
    Zen,
    Provider,
    Model,
    Reasoning,
    SignedOut,
    LoginHint,
    NotSaved,
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
    /// What a click on this field does; `None` is not clickable.
    action: Option<StatusClick>,
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
        action: click_action(key),
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

fn context_rank_for_percent(percent: f64) -> u8 {
    if percent >= CONTEXT_WARNING_PERCENT {
        RANK_CONTEXT_URGENT
    } else {
        RANK_CONTEXT
    }
}

/// Resolve context chrome and drop rank in one pass so style and packing stay aligned.
fn resolve_context_field(state: &StatusLineState) -> Option<(String, Style, u8)> {
    let context = state.context_usage.as_ref()?;
    let window = resolved_context_window(Some(context), state.model_metadata.as_ref());
    let Some(tokens) = context.tokens else {
        return match context.source {
            // Unknown after compaction is a real gap, not ambient chrome.
            ContextUsageSource::UnknownAfterCompaction => {
                Some(("?".into(), Theme::warning(), RANK_CONTEXT_URGENT))
            }
            ContextUsageSource::Estimated | ContextUsageSource::ProviderReported => None,
        };
    };
    // Without a known limit there is no fill percent; still show consumption.
    let Some(window) = window else {
        return Some((format_token_count(tokens), Theme::dim(), RANK_CONTEXT));
    };
    let percent = context_fill_percent(tokens, window);
    Some((
        format!("{} ({percent:.1}%)", format_token_count(tokens)),
        context_usage_style(percent),
        context_rank_for_percent(percent),
    ))
}

fn statusline_lines(
    state: &StatusLineState,
    width: usize,
) -> (Vec<Line<'static>>, Vec<StatusFieldHit>) {
    let cwd_path = compact_cwd(&state.cwd);
    let (bottom_left, bottom_right) = pack_bottom_status(state, width);
    let (fields_row, hits) = status_fields_line(&bottom_left, &bottom_right, width, state.hovered);
    let lines = vec![
        render_cwd_row(
            &cwd_path,
            state.branch.as_deref(),
            state.cwd_extra.as_ref(),
            width,
        ),
        fields_row,
    ];
    debug_assert_eq!(lines.len(), FIELDS_ROW + 1);
    (lines, hits)
}

/// Bottom row layout with an explicit field hierarchy.
///
/// Sides:
/// - left: `not saved · context · cost · rate`
/// - right identity: `permission · zen · provider · model · reasoning`
///
/// Drop order when width is scarce (first dropped first):
/// 1. reasoning
/// 2. generation rate
/// 3. zen indicator
/// 4. provider label
/// 5. session cost
/// 6. context usage (ambient; promotes above model at warning/critical fill, rank 65)
/// 7. model id
/// 8. unsaved-session warning
/// 9. permission mode
/// 10. signed-out warning (kept last)
///
/// Warning and critical fill share the urgent rank; only style differs at 90%.
fn pack_bottom_status(
    state: &StatusLineState,
    width: usize,
) -> (Vec<StatusField>, Vec<StatusField>) {
    let mut fields = bottom_fields(state);
    drop_to_width(&mut fields, width);
    split_sides(fields)
}

fn bottom_fields(state: &StatusLineState) -> Vec<StatusField> {
    let mut fields = Vec::with_capacity(9);

    if state.not_saved {
        fields.push(field(
            FieldKey::NotSaved,
            Side::Left,
            RANK_NOT_SAVED,
            0,
            "not saved",
            Theme::warning(),
        ));
    }

    if let Some((text, style, rank)) = resolve_context_field(state) {
        fields.push(field(FieldKey::Context, Side::Left, rank, 0, text, style));
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

    if state.zen_mode {
        fields.push(field(
            FieldKey::Zen,
            Side::Right,
            RANK_ZEN,
            2,
            "zen",
            Theme::dim(),
        ));
    }

    if !state.signed_in {
        // Naming the configured model would promise a turn the session cannot
        // run, so the row points at the fix instead.
        fields.push(field(
            FieldKey::SignedOut,
            Side::Right,
            RANK_SIGNED_OUT,
            3,
            "not signed in",
            Theme::warning(),
        ));
        fields.push(field(
            FieldKey::LoginHint,
            Side::Right,
            RANK_LOGIN_HINT,
            4,
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
            3,
            provider,
            Theme::dim(),
        ));
    }

    let model = state.model_name.as_deref().unwrap_or(&state.model);
    let model = if state.fast_mode_active {
        format!("{model} (fast)")
    } else {
        model.to_owned()
    };
    if !model.is_empty() {
        fields.push(field(
            FieldKey::Model,
            Side::Right,
            RANK_MODEL,
            4,
            model,
            Theme::dim(),
        ));
    }

    if state.reasoning_configurable {
        fields.push(field(
            FieldKey::Reasoning,
            Side::Right,
            RANK_REASONING,
            5,
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

fn status_cost(state: &StatusLineState) -> Option<String> {
    let main_cost_micros = state
        .usage
        .as_ref()
        .and_then(|usage| resolved_usage_cost_usd_micros(usage, state.model_metadata.as_ref()));
    session_total_cost_usd_micros(main_cost_micros, state.extra_cost_usd_micros).map(format_usd)
}

fn render_cwd_row(
    path: &str,
    branch: Option<&str>,
    extra: Option<&CwdExtra>,
    width: usize,
) -> Line<'static> {
    let extra_text = extra.map(|extra| extra.text.as_str());
    let (left, show_extra) = fit_cwd_row(path, branch, extra_text, width);
    let mut spans = vec![Span::styled(left, Theme::dim())];
    if let Some(extra) = extra.filter(|_| show_extra) {
        spans.push(Span::styled(extra.text.clone(), extra.style()));
    }
    Line::from(spans)
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
