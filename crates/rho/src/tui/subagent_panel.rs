use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::{activity, theme::Theme};
use crate::{
    subagent::{self, RunState},
    tools::agent::SubagentManager,
};

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SubagentRowState {
    #[default]
    Idle,
    Hovered,
    Pressed,
}

/// A clickable subagent row resolved from a pointer position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SubagentAttachTarget {
    pub(super) run_id: String,
    pub(super) agent_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SubagentPanel {
    agents: Vec<RunningSubagent>,
    hovered_run_id: Option<String>,
    pressed_run_id: Option<String>,
}

impl SubagentPanel {
    pub(super) fn update(&mut self, manager: Option<&SubagentManager>) -> bool {
        let agents = manager
            .map(SubagentManager::list)
            .unwrap_or_default()
            .into_iter()
            .filter(|snapshot| !snapshot.done && !snapshot.status.state.is_terminal())
            .map(|snapshot| RunningSubagent {
                id: snapshot.id,
                agent_id: snapshot.agent_id,
                title: snapshot.title,
                state: snapshot.status.state,
                last_activity: snapshot.status.last_activity,
                elapsed_seconds: snapshot.elapsed.as_secs(),
            })
            .collect();
        self.replace_agents(agents)
    }

    fn replace_agents(&mut self, agents: Vec<RunningSubagent>) -> bool {
        if self.agents == agents {
            return false;
        }
        self.agents = agents;
        let run_is_active = |run_id: &str| self.agents.iter().any(|agent| agent.id == run_id);
        if !self.hovered_run_id.as_deref().is_some_and(run_is_active) {
            self.hovered_run_id = None;
        }
        if !self.pressed_run_id.as_deref().is_some_and(run_is_active) {
            self.pressed_run_id = None;
        }
        true
    }

    pub(super) fn count(&self) -> usize {
        self.agents.len()
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
        let agents = self.visible_agents(activity::MAX_VISIBLE_RAIL_ROWS);
        let row_for = |run_id: &str| agents.iter().position(|agent| agent.id == run_id);
        if let Some(row) = self.pressed_run_id.as_deref().and_then(row_for) {
            return Some((row, SubagentRowState::Pressed));
        }
        self.hovered_run_id
            .as_deref()
            .and_then(row_for)
            .map(|row| (row, SubagentRowState::Hovered))
    }

    pub(super) fn attach_target(&self, run_id: &str) -> Option<SubagentAttachTarget> {
        self.agents
            .iter()
            .find(|agent| agent.id == run_id)
            .map(|agent| SubagentAttachTarget {
                run_id: agent.id.clone(),
                agent_id: agent.agent_id.clone(),
            })
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
    ) -> Option<SubagentAttachTarget> {
        if !area.contains(Position { x: column, y: row }) || area.height == 0 {
            return None;
        }
        let index = row.saturating_sub(area.y) as usize;
        let agents = self.visible_agents(area.height as usize);
        let agent = agents.get(index)?;
        Some(SubagentAttachTarget {
            run_id: agent.id.clone(),
            agent_id: agent.agent_id.clone(),
        })
    }

    pub(super) fn lines(
        &self,
        width: usize,
        height: usize,
        action_hint: &str,
        continues_below: bool,
    ) -> Vec<Line<'static>> {
        if self.agents.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let agents = self.visible_agents(height);
        let visible_count = agents.len();
        let mut lines = Vec::with_capacity(visible_count);
        for (index, agent) in agents.into_iter().enumerate() {
            let activity_text = match agent.state {
                RunState::Starting => "starting",
                RunState::Running => crate::title::activity_label(agent.last_activity.as_deref()),
                RunState::Ok | RunState::Error | RunState::Stopped => continue,
            };
            let connector =
                activity::tree_connector(index + 1 == visible_count && !continues_below);
            let row_state = self.row_state(agent);
            lines.push(agent_line(
                agent,
                activity_text,
                connector,
                width,
                row_state,
                action_hint,
            ));
        }
        lines
    }

    fn row_state(&self, agent: &RunningSubagent) -> SubagentRowState {
        if self.pressed_run_id.as_deref() == Some(agent.id.as_str()) {
            SubagentRowState::Pressed
        } else if self.hovered_run_id.as_deref() == Some(agent.id.as_str()) {
            SubagentRowState::Hovered
        } else {
            SubagentRowState::Idle
        }
    }

    fn running_agents(&self) -> Vec<&RunningSubagent> {
        self.agents
            .iter()
            .filter(|agent| matches!(agent.state, RunState::Starting | RunState::Running))
            .collect()
    }

    fn visible_agents(&self, height: usize) -> Vec<&RunningSubagent> {
        let limit = self
            .agents
            .len()
            .min(activity::MAX_VISIBLE_RAIL_ROWS)
            .min(height);
        self.running_agents().into_iter().take(limit).collect()
    }
}

fn agent_line(
    agent: &RunningSubagent,
    activity_text: &str,
    connector: &'static str,
    width: usize,
    row_state: SubagentRowState,
    action_hint: &str,
) -> Line<'static> {
    let elapsed = subagent::format_elapsed_secs(agent.elapsed_seconds);
    let trailing = match row_state {
        SubagentRowState::Idle => elapsed,
        SubagentRowState::Hovered | SubagentRowState::Pressed => action_hint.to_string(),
    };
    let row_style = Theme::subagent_row(row_state);
    activity::RailRow {
        connector,
        identity: rail_identity(agent, row_style),
        activity: activity_text.to_owned(),
        trailing,
        row_style,
    }
    .into_line(width)
}

fn rail_identity(agent: &RunningSubagent, row_style: ratatui::style::Style) -> Vec<Span<'static>> {
    let mut identity = vec![Span::styled(
        agent.agent_id.clone(),
        Theme::text_strong().patch(row_style),
    )];
    if let Some(title) = agent.title.as_deref().filter(|title| !title.is_empty()) {
        identity.push(Span::styled("  ", row_style));
        identity.push(Span::styled(
            title.to_owned(),
            Theme::dim().patch(row_style),
        ));
    }
    identity
}
