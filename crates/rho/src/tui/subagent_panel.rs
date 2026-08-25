use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::{activity, theme::Theme};
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

/// How a subagent row is being pointed at.
pub(super) type SubagentRowState = activity::RailRowState;

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SubagentPanel {
    agents: Vec<RunningSubagent>,
    terminal_seen: HashMap<String, Instant>,
    hovered_run_id: Option<String>,
    pressed_run_id: Option<String>,
}

impl SubagentPanel {
    pub(super) fn update(&mut self, manager: Option<&SubagentManager>, now: Instant) -> bool {
        let snapshots = manager.map(SubagentManager::list).unwrap_or_default();
        self.ingest(snapshots, now)
    }

    fn ingest(
        &mut self,
        snapshots: Vec<crate::tools::agent::SubagentSnapshot>,
        now: Instant,
    ) -> bool {
        let incoming: HashSet<String> = snapshots
            .iter()
            .map(|snapshot| snapshot.id.clone())
            .collect();
        self.terminal_seen.retain(|id, _| incoming.contains(id));
        let previously_shown: HashSet<String> =
            self.agents.iter().map(|agent| agent.id.clone()).collect();
        let agents = snapshots
            .into_iter()
            .filter_map(|snapshot| {
                let agent = RunningSubagent {
                    id: snapshot.id,
                    agent_id: snapshot.agent_id,
                    title: snapshot.title,
                    state: snapshot.status.state,
                    last_activity: snapshot.status.last_activity,
                    elapsed_seconds: snapshot.elapsed.as_secs(),
                };
                self.keep_agent(&agent, &previously_shown, now)
                    .then_some(agent)
            })
            .collect();
        self.replace_agents(agents)
    }

    fn keep_agent(
        &mut self,
        agent: &RunningSubagent,
        previously_shown: &HashSet<String>,
        now: Instant,
    ) -> bool {
        if is_live(agent.state) {
            self.terminal_seen.remove(&agent.id);
            return true;
        }
        if !agent.state.is_terminal() {
            return false;
        }
        if let Some(&first_seen) = self.terminal_seen.get(&agent.id) {
            return activity::linger_active(first_seen, now, linger_for_agent(agent.state));
        }
        if previously_shown.contains(&agent.id) {
            self.terminal_seen.insert(agent.id.clone(), now);
            return true;
        }
        false
    }

    fn replace_agents(&mut self, agents: Vec<RunningSubagent>) -> bool {
        if self.agents == agents {
            return false;
        }
        self.agents = agents;
        let overflow_active = self.agents.len() > activity::MAX_VISIBLE_RAIL_ROWS;
        let run_is_active = |run_id: &str| {
            (run_id == OPEN_ATTACH_PICKER_ID && overflow_active)
                || self
                    .agents
                    .iter()
                    .any(|agent| agent.id == run_id && is_live(agent.state))
        };
        if !self.hovered_run_id.as_deref().is_some_and(run_is_active) {
            self.hovered_run_id = None;
        }
        if !self.pressed_run_id.as_deref().is_some_and(run_is_active) {
            self.pressed_run_id = None;
        }
        true
    }

    pub(super) fn count(&self) -> usize {
        self.running_agents().len()
    }

    pub(super) fn is_active(&self) -> bool {
        !self.agents.is_empty()
    }

    pub(super) fn desired_height(&self) -> usize {
        self.agents.len().min(activity::MAX_VISIBLE_RAIL_ROWS)
    }

    pub(super) fn clear_pointer_state(&mut self) {
        self.hovered_run_id = None;
        self.pressed_run_id = None;
    }

    /// Returns whether the hovered run changed.
    pub(super) fn set_hovered(&mut self, run_id: Option<&str>) -> bool {
        if self.hovered_run_id.as_deref() == run_id {
            return false;
        }
        self.hovered_run_id = run_id.map(str::to_owned);
        true
    }

    /// Returns whether the pressed run changed.
    pub(super) fn set_pressed(&mut self, run_id: Option<&str>) -> bool {
        if self.pressed_run_id.as_deref() == run_id {
            return false;
        }
        self.pressed_run_id = run_id.map(str::to_owned);
        true
    }

    pub(super) fn pressed_run_id(&self) -> Option<&str> {
        self.pressed_run_id.as_deref()
    }

    pub(super) fn highlighted_row(&self) -> Option<(usize, SubagentRowState)> {
        let rows = self.visible_rows(activity::MAX_VISIBLE_RAIL_ROWS);
        let row_for = |run_id: &str| {
            rows.iter().position(|row| match row {
                VisibleSubagentRow::Agent(agent) => agent.id == run_id,
                VisibleSubagentRow::Overflow { .. } => run_id == OPEN_ATTACH_PICKER_ID,
            })
        };
        if let Some(row) = self.pressed_run_id.as_deref().and_then(row_for) {
            return Some((row, SubagentRowState::Pressed));
        }
        self.hovered_run_id
            .as_deref()
            .and_then(row_for)
            .map(|row| (row, SubagentRowState::Hovered))
    }

    pub(super) fn candidates(&self) -> Vec<super::attach_picker::AttachCandidate> {
        self.running_agents()
            .into_iter()
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
    ) -> Option<SubagentPointerTarget> {
        if !area.contains(Position { x: column, y: row }) || area.height == 0 {
            return None;
        }
        let index = row.saturating_sub(area.y) as usize;
        match self.visible_rows(area.height as usize).get(index)? {
            VisibleSubagentRow::Agent(agent) if is_live(agent.state) => {
                Some(SubagentPointerTarget::Run(SubagentAttachTarget {
                    run_id: agent.id.clone(),
                    agent_id: agent.agent_id.clone(),
                }))
            }
            VisibleSubagentRow::Overflow { .. } => Some(SubagentPointerTarget::OpenAttachPicker),
            VisibleSubagentRow::Agent(_) => None,
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
        if self.agents.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let rows = self.visible_rows_at(height, now);
        let visible_count = rows.len();
        let mut lines = Vec::with_capacity(visible_count);
        for (index, row) in rows.into_iter().enumerate() {
            let connector =
                activity::tree_connector(index + 1 == visible_count && !continues_below);
            lines.push(match row {
                VisibleSubagentRow::Agent(agent) => {
                    let (activity_text, activity_style) = agent_activity(agent);
                    agent_line(
                        agent,
                        activity_text,
                        activity_style,
                        connector,
                        width,
                        self.row_state(agent),
                        action_hint,
                    )
                }
                VisibleSubagentRow::Overflow { hidden } => overflow_line(
                    hidden,
                    connector,
                    width,
                    self.overflow_row_state(),
                    action_hint,
                ),
            });
        }
        lines
    }

    fn row_state(&self, agent: &RunningSubagent) -> SubagentRowState {
        if !is_live(agent.state) {
            return SubagentRowState::Idle;
        }
        if self.pressed_run_id.as_deref() == Some(agent.id.as_str()) {
            SubagentRowState::Pressed
        } else if self.hovered_run_id.as_deref() == Some(agent.id.as_str()) {
            SubagentRowState::Hovered
        } else {
            SubagentRowState::Idle
        }
    }

    fn overflow_row_state(&self) -> SubagentRowState {
        if self.pressed_run_id.as_deref() == Some(OPEN_ATTACH_PICKER_ID) {
            SubagentRowState::Pressed
        } else if self.hovered_run_id.as_deref() == Some(OPEN_ATTACH_PICKER_ID) {
            SubagentRowState::Hovered
        } else {
            SubagentRowState::Idle
        }
    }

    fn running_agents(&self) -> Vec<&RunningSubagent> {
        self.agents
            .iter()
            .filter(|agent| is_live(agent.state))
            .collect()
    }

    fn visible_rows(&self, height: usize) -> Vec<VisibleSubagentRow<'_>> {
        self.visible_rows_at(height, Instant::now())
    }

    fn visible_rows_at(&self, height: usize, now: Instant) -> Vec<VisibleSubagentRow<'_>> {
        let agents: Vec<&RunningSubagent> = self
            .agents
            .iter()
            .filter(|agent| self.row_visible(agent, now))
            .collect();
        let (indices, hidden) = activity::select_capped_rail_rows(
            &agents,
            height,
            |agent| is_live(agent.state),
            |agent| agent.state == RunState::Error,
        );
        let mut rows: Vec<VisibleSubagentRow<'_>> = indices
            .into_iter()
            .map(|index| VisibleSubagentRow::Agent(agents[index]))
            .collect();
        if let Some(hidden) = hidden {
            rows.push(VisibleSubagentRow::Overflow { hidden });
        }
        rows
    }

    fn row_visible(&self, agent: &RunningSubagent, now: Instant) -> bool {
        if is_live(agent.state) {
            return true;
        }
        self.terminal_seen.get(&agent.id).is_some_and(|first| {
            activity::linger_active(*first, now, linger_for_agent(agent.state))
        })
    }
}

enum VisibleSubagentRow<'a> {
    Agent(&'a RunningSubagent),
    Overflow { hidden: usize },
}

fn agent_line(
    agent: &RunningSubagent,
    activity_text: String,
    activity_style: ratatui::style::Style,
    connector: &'static str,
    width: usize,
    row_state: SubagentRowState,
    action_hint: &str,
) -> Line<'static> {
    let elapsed = subagent::format_elapsed_secs(agent.elapsed_seconds);
    let trailing = match row_state {
        SubagentRowState::Idle => elapsed,
        SubagentRowState::Hovered | SubagentRowState::Pressed => {
            format!("⏎ {action_hint} · {elapsed}")
        }
    };
    let row_style = Theme::subagent_row(row_state);
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
    row_state: SubagentRowState,
    action_hint: &str,
) -> Line<'static> {
    let trailing = match row_state {
        SubagentRowState::Idle => String::new(),
        SubagentRowState::Hovered | SubagentRowState::Pressed => format!("⏎ {action_hint}"),
    };
    let row_style = Theme::subagent_row(row_state);
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

fn is_live(state: RunState) -> bool {
    matches!(state, RunState::Starting | RunState::Running)
}

fn linger_for_agent(state: RunState) -> Duration {
    match state {
        RunState::Error => activity::LINGER_FAIL,
        RunState::Ok | RunState::Stopped | RunState::Starting | RunState::Running => {
            activity::LINGER_OK
        }
    }
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
