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
/// Content width clamp shared by stacked activity-rail rows.
const MAX_RAIL_CONTENT_WIDTH: usize = 52;

/// One stacked activity-rail row. Identity styling stays at the call site.
pub(super) struct RailRow {
    pub(super) connector: &'static str,
    pub(super) identity: Vec<Span<'static>>,
    pub(super) activity: String,
    pub(super) trailing: String,
    pub(super) row_style: Style,
}

impl RailRow {
    pub(super) fn into_line(self, width: usize) -> Line<'static> {
        const SEPARATOR: &str = "  ·  ";
        const MIN_GAP: usize = 2;

        let connector_width = display_width(self.connector);
        let content_width = width
            .saturating_sub(connector_width)
            .min(MAX_RAIL_CONTENT_WIDTH);
        let identity_plain: String = self
            .identity
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let identity_width = display_width(&identity_plain);
        let separator_width = display_width(SEPARATOR);
        let trailing_width = display_width(&self.trailing);
        let fixed_width = identity_width + separator_width + MIN_GAP + trailing_width;
        let row_style = self.row_style;

        if fixed_width >= content_width {
            let detail = truncate_one_line(
                &format!(
                    "{identity_plain}{SEPARATOR}{}  {}",
                    self.activity, self.trailing
                ),
                content_width,
            );
            return Line::from(vec![
                Span::styled(self.connector, Theme::dim().patch(row_style)),
                Span::styled(detail, Theme::dim().patch(row_style)),
            ]);
        }

        let activity_width = content_width.saturating_sub(fixed_width);
        let activity = truncate_one_line(&self.activity, activity_width);
        let gap = " ".repeat(content_width.saturating_sub(
            identity_width + separator_width + display_width(&activity) + trailing_width,
        ));
        let mut spans = Vec::with_capacity(self.identity.len() + 5);
        spans.push(Span::styled(self.connector, Theme::dim().patch(row_style)));
        spans.extend(self.identity);
        spans.push(Span::styled(SEPARATOR, Theme::dim().patch(row_style)));
        spans.push(Span::styled(activity, Theme::text().patch(row_style)));
        spans.push(Span::styled(gap, row_style));
        spans.push(Span::styled(self.trailing, Theme::dim().patch(row_style)));
        Line::from(spans)
    }
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
        let rate_limited = matches!(
            self.reason.provider_error_kind(),
            Some(ProviderErrorKind::RateLimit)
        );
        match (
            rate_limited,
            self.reason.retry_after().filter(|delay| !delay.is_zero()),
        ) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActivityStatus {
    Parent {
        phase: ActivityPhase,
        retry: Option<ProviderRetryHint>,
    },
    Subagents(usize),
    ParentWithSubagents {
        phase: ActivityPhase,
        retry: Option<ProviderRetryHint>,
        subagent_count: usize,
    },
}

impl ActivityStatus {
    pub(super) fn from_parent_and_subagents(
        parent: Option<(ActivityPhase, Option<ProviderRetryHint>)>,
        subagent_count: usize,
    ) -> Option<Self> {
        match (parent, subagent_count) {
            (Some((phase, retry)), 0) => Some(Self::Parent { phase, retry }),
            (Some((phase, retry)), count) => Some(Self::ParentWithSubagents {
                phase,
                retry,
                subagent_count: count,
            }),
            (None, 0) => None,
            (None, count) => Some(Self::Subagents(count)),
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

fn activity_status_labels(status: ActivityStatus) -> Vec<String> {
    let spinner = LoadingSpinner::FRAMES[0];
    let subagent_count = match status {
        ActivityStatus::Parent { .. } => 0,
        ActivityStatus::Subagents(count)
        | ActivityStatus::ParentWithSubagents {
            subagent_count: count,
            ..
        } => count,
    };
    let agents = if subagent_count == 1 {
        "1 agent".into()
    } else {
        format!("{subagent_count} agents")
    };
    match status {
        ActivityStatus::Parent { phase, retry } => {
            let label = phase_label(phase, retry);
            vec![format!("{spinner} {label}"), spinner.into()]
        }
        ActivityStatus::ParentWithSubagents { phase, retry, .. } => {
            let label = phase_label(phase, retry);
            vec![
                format!("{spinner} {label}  ·  {agents}"),
                format!("{spinner} {label} · {subagent_count}"),
                format!("{spinner} {subagent_count}"),
                spinner.into(),
            ]
        }
        ActivityStatus::Subagents(_) => vec![
            format!("{spinner} {agents} working"),
            format!("{spinner} {subagent_count} agents"),
            format!("{spinner} {subagent_count}"),
            spinner.into(),
        ],
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
            Span::styled(rest.to_string(), Theme::dim()),
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
