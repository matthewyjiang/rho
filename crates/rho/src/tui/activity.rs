use std::time::{Duration, Instant};

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use rho_sdk::{ProviderErrorKind, ProviderStreamResetReason};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
};

/// Rows occupied by the spinner/jump activity rail.
pub(super) const ACTIVITY_RAIL_ROWS: usize = 1;
/// Blank breathing room kept above the rail while bottom-following.
pub(super) const ACTIVITY_CONTENT_GAP_ROWS: usize = 1;

/// Tree connector for stacked activity-rail rows.
pub(super) fn tree_connector(is_last: bool) -> &'static str {
    if is_last {
        "  └ "
    } else {
        "  ├ "
    }
}

/// Visible stacked rows shared by the subagent and process rails.
pub(super) const MAX_VISIBLE_RAIL_ROWS: usize = 2;

/// Agent-row identity prefix (glyph + space).
pub(super) const AGENT_GLYPH: &str = "◉ ";
/// Process-row identity prefix (glyph + space).
pub(super) const PROCESS_GLYPH: &str = "⚙ ";

/// How an activity-rail row is being pointed at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RailRowState {
    #[default]
    Idle,
    Hovered,
    Pressed,
}

/// Long enough to register the ✓ while reading; short enough not to squat rail
/// rows.
pub(super) const LINGER_OK: Duration = Duration::from_secs(2);
/// A failure verdict is the one thing the rail must not let you miss.
pub(super) const LINGER_FAIL: Duration = Duration::from_secs(5);

const _: () = assert!(
    LINGER_FAIL.as_secs() < crate::tools::RAIL_TERMINAL_RETENTION.as_secs(),
    "UI linger must drop a row before the manager forgets it"
);

/// One stacked activity-rail row. Identity styling stays at the call site.
pub(super) struct RailRow {
    pub(super) connector: &'static str,
    pub(super) identity: Vec<Span<'static>>,
    pub(super) activity: String,
    pub(super) activity_style: Style,
    pub(super) trailing: String,
    pub(super) trailing_style: Style,
    pub(super) row_style: Style,
}

impl RailRow {
    pub(super) fn into_line(self, width: usize) -> Line<'static> {
        const SEPARATOR: &str = "  ·  ";
        const MIN_GAP: usize = 2;

        let content_width = width.saturating_sub(display_width(self.connector));
        let row_style = self.row_style;
        let identity_style = self.identity_style();
        let mut spans = Vec::with_capacity(self.identity.len() + 5);
        spans.push(Span::styled(self.connector, Theme::dim().patch(row_style)));

        let trailing_reserve = if self.trailing.is_empty() {
            0
        } else {
            MIN_GAP + display_width(&self.trailing)
        };
        let identity_plain: String = self
            .identity
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let identity_width = display_width(&identity_plain);
        let identity_budget = content_width.saturating_sub(trailing_reserve);
        let mut used = if identity_width <= identity_budget {
            spans.extend(self.identity);
            identity_width
        } else {
            let identity = truncate_one_line(&identity_plain, identity_budget);
            let shown = display_width(&identity);
            spans.push(Span::styled(identity, identity_style.patch(row_style)));
            shown
        };

        let separator_width = display_width(SEPARATOR);
        let activity_budget =
            content_width.saturating_sub(used + trailing_reserve + separator_width);
        // Skip the middle column unless it can show more than a lone ellipsis.
        if !self.activity.is_empty() && activity_budget > 1 {
            let activity = truncate_one_line(&self.activity, activity_budget);
            if !activity.is_empty() {
                used += separator_width + display_width(&activity);
                spans.push(Span::styled(SEPARATOR, Theme::dim().patch(row_style)));
                spans.push(Span::styled(activity, row_style.patch(self.activity_style)));
            }
        }

        if !self.trailing.is_empty() {
            used += trailing_reserve;
            spans.push(Span::styled(" ".repeat(MIN_GAP), row_style));
            spans.push(Span::styled(
                self.trailing,
                row_style.patch(self.trailing_style),
            ));
        }

        let fill = content_width.saturating_sub(used);
        if fill > 0 {
            spans.push(Span::styled(" ".repeat(fill), row_style));
        }
        Line::from(spans)
    }

    fn identity_style(&self) -> Style {
        self.identity
            .first()
            .map(|span| span.style)
            .unwrap_or_else(Theme::dim)
    }
}

/// Hidden-row copy for a per-panel overflow summary.
pub(super) fn overflow_label(hidden: usize, singular: &str, plural: &str) -> String {
    if hidden == 1 {
        format!("1 more {singular}")
    } else {
        format!("{hidden} more {plural}")
    }
}

/// Indices to paint when a panel has more rows than `height` / the shared cap.
///
/// Live rows win over lingering rows. Lingering failures win over lingering
/// successes. Original order is preserved among the rows that remain. When
/// anything is hidden, the last visible slot is reserved for a summary.
pub(super) fn select_capped_rail_rows<T>(
    rows: &[T],
    height: usize,
    is_live: impl Fn(&T) -> bool,
    is_failure: impl Fn(&T) -> bool,
) -> (Vec<usize>, Option<usize>) {
    let cap = MAX_VISIBLE_RAIL_ROWS.min(height);
    if cap == 0 || rows.is_empty() {
        return (Vec::new(), None);
    }
    if rows.len() <= cap {
        return ((0..rows.len()).collect(), None);
    }
    let content_slots = cap.saturating_sub(1);
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&index| {
        let rank = if is_live(&rows[index]) {
            0_u8
        } else if is_failure(&rows[index]) {
            1
        } else {
            2
        };
        (rank, index)
    });
    order.truncate(content_slots);
    order.sort_unstable();
    let hidden = rows.len() - order.len();
    (order, Some(hidden))
}

/// Whether a terminal rail row should still occupy a slot.
pub(super) fn linger_active(first_seen: Instant, now: Instant, linger: Duration) -> bool {
    now.saturating_duration_since(first_seen) < linger
}

/// Transcript rows reserved under the history panel while bottom-following with
/// activity chrome. Manual scroll keeps the full panel so content can sit under
/// the overlay.
pub(super) fn bottom_follow_activity_inset(activity_active: bool, bottom_follow: bool) -> usize {
    if activity_active && bottom_follow {
        ACTIVITY_RAIL_ROWS + ACTIVITY_CONTENT_GAP_ROWS
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActivityPhase {
    #[default]
    Starting,
    WaitingForProvider,
    Thinking,
    Responding,
    PreparingTool,
    RunningTool,
    RetryingProvider,
    ConnectingMcp,
    Compacting,
    WaitingForApproval,
    WaitingForInput,
}

impl ActivityPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::WaitingForProvider => "waiting for provider",
            Self::Thinking => "thinking",
            Self::Responding => "responding",
            Self::PreparingTool => "preparing tool",
            Self::RunningTool => "running tool",
            Self::RetryingProvider => "retrying provider",
            Self::ConnectingMcp => "connecting MCP servers",
            Self::Compacting => "compacting context",
            Self::WaitingForApproval => "waiting for approval",
            Self::WaitingForInput => "waiting for input",
        }
    }
}

/// Structured provider-retry context for spinner/status copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProviderRetryHint {
    pub(super) reason: ProviderStreamResetReason,
}

impl ProviderRetryHint {
    pub(super) fn status_label(self) -> String {
        let (kind, retry_after) = match self.reason {
            ProviderStreamResetReason::RetryableFailure { kind, retry_after } => {
                (Some(kind), retry_after.filter(|delay| !delay.is_zero()))
            }
            ProviderStreamResetReason::InvalidResponse => (None, None),
            // Required while the SDK enum stays `#[non_exhaustive]`.
            _ => (None, None),
        };
        let rate_limited = kind == Some(ProviderErrorKind::RateLimit);
        match (rate_limited, retry_after) {
            (true, Some(delay)) => format!(
                "rate limited · retry in {}",
                rho_sdk::format_retry_after(delay)
            ),
            (true, None) => "rate limited · retrying".into(),
            (false, Some(delay)) => format!(
                "retrying provider · in {}",
                rho_sdk::format_retry_after(delay)
            ),
            (false, None) => "retrying provider response".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct BackgroundCounts {
    pub(super) subagent_count: usize,
    pub(super) job_count: usize,
}

impl BackgroundCounts {
    fn is_empty(self) -> bool {
        self.subagent_count == 0 && self.job_count == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActivityStatus {
    Parent {
        phase: ActivityPhase,
        retry: Option<ProviderRetryHint>,
        background: BackgroundCounts,
    },
    Background(BackgroundCounts),
    Linger,
}

impl ActivityStatus {
    pub(super) fn from_parent_and_background(
        parent: Option<(ActivityPhase, Option<ProviderRetryHint>)>,
        background: BackgroundCounts,
        rail_occupied: bool,
    ) -> Option<Self> {
        match (parent, background.is_empty(), rail_occupied) {
            (Some((phase, retry)), _, _) => Some(Self::Parent {
                phase,
                retry,
                background,
            }),
            (None, false, _) => Some(Self::Background(background)),
            (None, true, true) => Some(Self::Linger),
            (None, true, false) => None,
        }
    }
}

fn phase_label(phase: ActivityPhase, retry: Option<ProviderRetryHint>) -> String {
    if matches!(phase, ActivityPhase::RetryingProvider) {
        if let Some(retry) = retry {
            return retry.status_label();
        }
    }
    phase.label().to_string()
}

fn counted_noun(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn activity_status_labels(status: ActivityStatus) -> Vec<String> {
    let spinner = LoadingSpinner::FRAMES[0];
    match status {
        ActivityStatus::Parent {
            phase,
            retry,
            background,
        } => parent_background_rungs(spinner, &phase_label(phase, retry), background),
        ActivityStatus::Background(background) => background_only_rungs(spinner, background),
        ActivityStatus::Linger => vec![spinner.into()],
    }
}

fn parent_background_rungs(spinner: &str, label: &str, counts: BackgroundCounts) -> Vec<String> {
    if counts.is_empty() {
        return vec![format!("{spinner} {label}"), spinner.into()];
    }
    let wide = background_wide(counts);
    if counts.subagent_count > 0 && counts.job_count > 0 {
        vec![
            format!("{spinner} {label}  ·  {wide}"),
            format!(
                "{spinner} {label} · {}+{}",
                counts.subagent_count, counts.job_count
            ),
            format!("{spinner} {}+{}", counts.subagent_count, counts.job_count),
            spinner.into(),
        ]
    } else {
        let n = counts.subagent_count.max(counts.job_count);
        vec![
            format!("{spinner} {label}  ·  {wide}"),
            format!("{spinner} {label} · {n}"),
            format!("{spinner} {n}"),
            spinner.into(),
        ]
    }
}

fn background_only_rungs(spinner: &str, counts: BackgroundCounts) -> Vec<String> {
    let wide = background_wide(counts);
    if counts.subagent_count > 0 && counts.job_count > 0 {
        vec![
            format!("{spinner} {wide}"),
            format!("{spinner} {}+{}", counts.subagent_count, counts.job_count),
            spinner.into(),
        ]
    } else if counts.subagent_count > 0 {
        vec![
            format!("{spinner} {wide} working"),
            format!("{spinner} {wide}"),
            format!("{spinner} {}", counts.subagent_count),
            spinner.into(),
        ]
    } else {
        vec![
            format!("{spinner} {wide} running"),
            format!("{spinner} {wide}"),
            format!("{spinner} {}", counts.job_count),
            spinner.into(),
        ]
    }
}

fn background_wide(counts: BackgroundCounts) -> String {
    match (counts.subagent_count > 0, counts.job_count > 0) {
        (true, true) => format!(
            "{} · {}",
            counted_noun(counts.subagent_count, "agent", "agents"),
            counted_noun(counts.job_count, "job", "jobs")
        ),
        (true, false) => counted_noun(counts.subagent_count, "agent", "agents"),
        (false, true) => counted_noun(counts.job_count, "job", "jobs"),
        (false, false) => String::new(),
    }
}

/// Status ladder first, then a trailing timer on the widest label.
///
/// Narrow widths drop the timer before degrading the status text, matching
/// stacked rails that keep elapsed as a trailing column.
fn activity_label(available: usize, status: ActivityStatus, elapsed: Option<Duration>) -> String {
    let first_frame = LoadingSpinner::FRAMES[0];
    let labels = activity_status_labels(status);
    let mut candidates = Vec::with_capacity(labels.len() + usize::from(elapsed.is_some()));
    if let Some(elapsed) = elapsed {
        candidates.push(format!(
            "{} · {}",
            labels[0],
            super::goal::format_elapsed_with(
                elapsed,
                super::goal::ElapsedPrecision::TenthsUnderMinute
            )
        ));
    }
    candidates.extend(labels);
    candidates
        .into_iter()
        .find(|label| display_width(label) <= available)
        .unwrap_or_else(|| first_frame.chars().take(available).collect())
}

#[derive(Clone, Debug, Default)]
pub(super) struct LoadingSpinner {
    started_at: Option<Instant>,
}

impl LoadingSpinner {
    const FRAMES: [&'static str; 8] = ["⠙", "⠋", "⠇", "⡆", "⣄", "⣠", "⢰", "⠸"];
    pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(95);

    pub(super) fn start(&mut self) {
        self.started_at = Some(Instant::now());
    }

    pub(super) fn start_if_needed(&mut self) {
        if self.started_at.is_none() {
            self.start();
        }
    }

    pub(super) fn stop(&mut self) {
        self.started_at = None;
    }

    pub(super) fn frame_since(started_at: Instant, now: Instant) -> &'static str {
        let interval_ms = Self::FRAME_INTERVAL.as_millis().max(1);
        let frame = now
            .saturating_duration_since(started_at)
            .as_millis()
            .checked_div(interval_ms)
            .unwrap_or(0) as usize;
        Self::FRAMES[frame % Self::FRAMES.len()]
    }

    fn frame_at(&self, now: Instant) -> &'static str {
        let Some(started_at) = self.started_at else {
            return Self::FRAMES[0];
        };
        Self::frame_since(started_at, now)
    }

    pub(super) fn elapsed_at(&self, now: Instant) -> Option<Duration> {
        self.started_at
            .map(|started_at| now.saturating_duration_since(started_at))
    }

    pub(super) fn line(
        &self,
        now: Instant,
        available: usize,
        status: ActivityStatus,
    ) -> Line<'static> {
        let label = activity_label(available, status, self.elapsed_at(now));
        let Some(rest) = label.strip_prefix(Self::FRAMES[0]) else {
            return Line::default();
        };
        Line::from(vec![
            Span::styled(self.frame_at(now), Theme::accent()),
            Span::styled(rest.to_string(), Theme::activity_rail_dim()),
        ])
    }
}

/// What the jump-to-bottom chip should communicate beyond navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JumpChipState {
    /// Plain navigation affordance while reading history.
    Neutral,
    /// The last provider turn finished while the user was scrolled away.
    ResponseReady,
    /// An approval prompt is blocking the agent.
    ApprovalNeeded,
    /// A questionnaire or input form is blocking the agent.
    InputNeeded,
}

impl JumpChipState {
    pub(super) fn is_attention(self) -> bool {
        self != Self::Neutral
    }

    /// (full label, compact label) used by [`jump_to_bottom_text`].
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Neutral => ("jump to bottom", "bottom"),
            Self::ResponseReady => ("response ready", "ready"),
            Self::ApprovalNeeded => ("approval needed", "ask"),
            Self::InputNeeded => ("input needed", "input"),
        }
    }
}

pub(super) fn jump_to_bottom_text(
    width: usize,
    binding: &str,
    alongside_activity: bool,
    state: JumpChipState,
) -> String {
    let (full_action, compact_action) = state.labels();
    let full = format!("↓ {full_action}  {binding}");
    let compact = format!("↓ {compact_action} {binding}");
    let shortcut = format!("↓ {binding}");
    // Compact activity after the timer has already dropped. The live label
    // degrades elapsed first, so the jump chip only needs this floor.
    let activity_width = usize::from(alongside_activity)
        * (display_width(LoadingSpinner::FRAMES[0]) + display_width(" 0") + 1);
    let available = width.saturating_sub(activity_width);

    if display_width(&full) <= available {
        full
    } else if display_width(&compact) <= available {
        compact
    } else if display_width(&shortcut) <= width {
        shortcut
    } else {
        truncate_one_line(&shortcut, width)
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
