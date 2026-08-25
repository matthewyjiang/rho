use std::time::{Duration, Instant};

use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::{
    activity,
    linger_rail::{LingerRail, RailHit, RailItem, RailPointerPolicy},
    theme::Theme,
};
use crate::{
    subagent::{self, RunState},
    title::activity_label,
    tools::agent::SubagentManager,
};

const OPEN_ATTACH_PICKER_ID: &str = "/attach";

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunningSubagent {
    id: String,
    agent_id: String,
    title: Option<String>,
    state: RunState,
    last_activity: Option<String>,
    elapsed_seconds: u64,
}

/// A clickable subagent row resolved from a pointer position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SubagentAttachTarget {
    pub(super) run_id: String,
    pub(super) agent_id: String,
}

/// Pointer hit on the subagent rail: attach one run, or open `/attach`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SubagentPointerTarget {
    Run(SubagentAttachTarget),
    OpenAttachPicker,
}

impl SubagentPointerTarget {
    pub(super) fn pointer_id(&self) -> &str {
        match self {
            Self::Run(target) => target.run_id.as_str(),
            Self::OpenAttachPicker => OPEN_ATTACH_PICKER_ID,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct SubagentPanel {
    rail: LingerRail<RunningSubagent>,
}

impl Default for SubagentPanel {
    fn default() -> Self {
        Self {
            rail: LingerRail::new(RailPointerPolicy::LiveAndOverflow {
                overflow_id: OPEN_ATTACH_PICKER_ID,
            }),
        }
    }
}

impl SubagentPanel {
    pub(super) fn update(&mut self, manager: Option<&SubagentManager>, now: Instant) -> bool {
        let snapshots = manager
            .map(SubagentManager::rail_summaries)
            .unwrap_or_default();
        self.ingest(snapshots, now)
    }

    pub(super) fn ingest(
        &mut self,
        snapshots: Vec<crate::tools::agent::SubagentSnapshot>,
        now: Instant,
    ) -> bool {
        let agents = snapshots
            .into_iter()
            .map(|snapshot| RunningSubagent {
                id: snapshot.id,
                agent_id: snapshot.agent_id,
                title: snapshot.title,
                state: snapshot.status.state,
                last_activity: snapshot.status.last_activity,
                elapsed_seconds: snapshot.elapsed.as_secs(),
            })
            .collect();
        self.rail.ingest(agents, now)
    }

    pub(super) fn count(&self) -> usize {
        self.rail.live_count()
    }

    pub(super) fn is_active(&self) -> bool {
        self.rail.is_active()
    }

    pub(super) fn desired_height(&self) -> usize {
        self.rail.desired_height()
    }

    pub(super) fn clear_pointer_state(&mut self) {
        self.rail.clear_pointer_state();
    }

    pub(super) fn clear_pressed(&mut self) {
        self.rail.clear_pressed();
    }

    /// Returns whether the hovered run changed.
    pub(super) fn set_hovered(&mut self, run_id: Option<&str>) -> bool {
        self.rail.set_hovered(run_id)
    }

    /// Returns whether the pressed run changed.
    pub(super) fn set_pressed(&mut self, run_id: Option<&str>) -> bool {
        self.rail.set_pressed(run_id)
    }

    pub(super) fn pressed_run_id(&self) -> Option<&str> {
        self.rail.pressed_id()
    }

    pub(super) fn highlighted_row(&self, now: Instant) -> Option<(usize, activity::RailRowState)> {
        self.rail.highlighted_row(now)
    }

    pub(super) fn candidates(&self) -> Vec<super::attach_picker::AttachCandidate> {
        self.rail
            .live_items()
            .map(|agent| super::attach_picker::AttachCandidate {
                run_id: agent.id.clone(),
                agent_id: agent.agent_id.clone(),
                title: agent.title.clone(),
                last_activity: agent.last_activity.clone(),
                state: agent.state,
                elapsed_seconds: agent.elapsed_seconds,
            })
            .collect()
    }

    pub(super) fn attach_target_at(
        &self,
        area: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) -> Option<SubagentPointerTarget> {
        match self.rail.hit_at(area, column, row, now)? {
            RailHit::Overflow => Some(SubagentPointerTarget::OpenAttachPicker),
            RailHit::Item(run_id) => self
                .rail
                .items()
                .iter()
                .find(|agent| agent.id == run_id)
                .map(|agent| {
                    SubagentPointerTarget::Run(SubagentAttachTarget {
                        run_id: agent.id.clone(),
                        agent_id: agent.agent_id.clone(),
                    })
                }),
        }
    }

    pub(super) fn lines(
        &self,
        width: usize,
        height: usize,
        action_hint: &str,
        continues_below: bool,
        now: Instant,
    ) -> Vec<Line<'static>> {
        if !self.rail.is_active() || width == 0 || height == 0 {
            return Vec::new();
        }

        let (rows, hidden) = self.rail.visible(height, now);
        let visible_count = rows.len() + usize::from(hidden.is_some());
        let mut lines = Vec::with_capacity(visible_count);
        for (index, agent) in rows.into_iter().enumerate() {
            let last = index + 1 == visible_count && !continues_below;
            let (activity_text, activity_style) = agent_activity(agent);
            lines.push(agent_line(
                agent,
                activity_text,
                activity_style,
                activity::tree_connector(last),
                width,
                self.rail.row_state(&agent.id, agent.state.is_live()),
                action_hint,
            ));
        }
        if let Some(hidden) = hidden {
            lines.push(overflow_line(
                hidden,
                activity::tree_connector(!continues_below),
                width,
                self.rail.overflow_row_state(),
                action_hint,
            ));
        }
        lines
    }
}

impl RailItem for RunningSubagent {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_live(&self) -> bool {
        self.state.is_live()
    }

    fn is_failure(&self) -> bool {
        self.state == RunState::Error
    }

    fn linger(&self) -> Duration {
        match self.state {
            RunState::Error => activity::LINGER_FAIL,
            RunState::Ok | RunState::Stopped | RunState::Starting | RunState::Running => {
                activity::LINGER_OK
            }
        }
    }
}

fn agent_line(
    agent: &RunningSubagent,
    activity_text: String,
    activity_style: ratatui::style::Style,
    connector: &'static str,
    width: usize,
    row_state: activity::RailRowState,
    action_hint: &str,
) -> Line<'static> {
    let elapsed = subagent::format_elapsed_secs(agent.elapsed_seconds);
    let trailing = match row_state {
        activity::RailRowState::Idle => elapsed,
        activity::RailRowState::Hovered | activity::RailRowState::Pressed => {
            format!("⏎ {action_hint} · {elapsed}")
        }
    };
    let row_style = Theme::activity_rail_row(row_state);
    activity::RailRow {
        connector,
        identity: rail_identity(agent, row_style),
        activity: activity_text,
        activity_style,
        trailing,
        trailing_style: Theme::dim(),
        row_style,
    }
    .into_line(width)
}

fn overflow_line(
    hidden: usize,
    connector: &'static str,
    width: usize,
    row_state: activity::RailRowState,
    action_hint: &str,
) -> Line<'static> {
    let trailing = match row_state {
        activity::RailRowState::Idle => String::new(),
        activity::RailRowState::Hovered | activity::RailRowState::Pressed => {
            format!("⏎ {action_hint}")
        }
    };
    let row_style = Theme::activity_rail_row(row_state);
    activity::RailRow {
        connector,
        identity: vec![Span::styled(
            activity::overflow_label(hidden, "agent", "agents"),
            Theme::dim().patch(row_style),
        )],
        activity: "/attach".into(),
        activity_style: Theme::dim(),
        trailing,
        trailing_style: Theme::dim(),
        row_style,
    }
    .into_line(width)
}

fn rail_identity(agent: &RunningSubagent, row_style: ratatui::style::Style) -> Vec<Span<'static>> {
    let mut identity = vec![
        Span::styled(activity::AGENT_GLYPH, Theme::text_strong().patch(row_style)),
        Span::styled(
            agent.agent_id.clone(),
            Theme::text_strong().patch(row_style),
        ),
    ];
    if let Some(title) = agent.title.as_deref().filter(|title| !title.is_empty()) {
        identity.push(Span::styled("  ", row_style));
        identity.push(Span::styled(
            title.to_owned(),
            Theme::dim().patch(row_style),
        ));
    }
    identity
}

fn agent_activity(agent: &RunningSubagent) -> (String, ratatui::style::Style) {
    match agent.state {
        RunState::Starting => ("starting".into(), Theme::text()),
        RunState::Running => (
            activity_label(agent.last_activity.as_deref()).to_owned(),
            Theme::text(),
        ),
        RunState::Ok => ("✓ done".into(), Theme::activity_rail_success()),
        RunState::Error => ("✗ error".into(), Theme::activity_rail_error()),
        RunState::Stopped => ("✗ stopped".into(), Theme::dim()),
    }
}

#[cfg(test)]
#[path = "subagent_panel_tests.rs"]
mod tests;
