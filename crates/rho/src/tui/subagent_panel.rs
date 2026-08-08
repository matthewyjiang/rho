use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
};
use crate::{
    subagent::{self, RunState},
    tools::agent::SubagentManager,
};

const MAX_VISIBLE_AGENTS: usize = 2;
const MAX_AGENT_CONTENT_WIDTH: usize = 52;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunningSubagent {
    id: String,
    agent_id: String,
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
        self.agents.len().min(MAX_VISIBLE_AGENTS)
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
        let agents = self.visible_agents(MAX_VISIBLE_AGENTS);
        let row_for = |run_id: &str| agents.iter().position(|agent| agent.id == run_id);
        if let Some(row) = self.pressed_run_id.as_deref().and_then(row_for) {
            return Some((row, SubagentRowState::Pressed));
        }
        self.hovered_run_id
            .as_deref()
            .and_then(row_for)
            .map(|row| (row, SubagentRowState::Hovered))
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
    ) -> Vec<Line<'static>> {
        if self.agents.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let agents = self.visible_agents(height);
        let visible_count = agents.len();
        let mut lines = Vec::with_capacity(visible_count);
        for (index, agent) in agents.into_iter().enumerate() {
            let activity = match agent.state {
                RunState::Starting => "starting",
                RunState::Running => activity_label(agent.last_activity.as_deref()),
                RunState::Ok | RunState::Error | RunState::Stopped => continue,
            };
            let connector = if index + 1 == visible_count {
                "  └ "
            } else {
                "  ├ "
            };
            let row_state = self.row_state(agent);
            lines.push(agent_line(
                agent,
                activity,
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

    fn visible_agents(&self, height: usize) -> Vec<&RunningSubagent> {
        let limit = self.agents.len().min(MAX_VISIBLE_AGENTS).min(height);
        self.agents
            .iter()
            .filter(|agent| matches!(agent.state, RunState::Starting | RunState::Running))
            .take(limit)
            .collect()
    }
}

fn agent_line(
    agent: &RunningSubagent,
    activity: &str,
    connector: &'static str,
    width: usize,
    row_state: SubagentRowState,
    action_hint: &str,
) -> Line<'static> {
    const SEPARATOR: &str = "  ·  ";
    const MIN_GAP: usize = 2;

    let connector_width = display_width(connector);
    let content_width = width
        .saturating_sub(connector_width)
        .min(MAX_AGENT_CONTENT_WIDTH);
    let identity_width = display_width(&agent.agent_id) + 2 + display_width(&agent.id);
    let separator_width = display_width(SEPARATOR);
    let elapsed = format_elapsed(agent.elapsed_seconds);
    let trailing = match row_state {
        SubagentRowState::Idle => elapsed,
        SubagentRowState::Hovered | SubagentRowState::Pressed => action_hint.to_string(),
    };
    let fixed_width = identity_width + separator_width + MIN_GAP + display_width(&trailing);
    let row_style = Theme::subagent_row(row_state);

    if fixed_width >= content_width {
        let detail = truncate_one_line(
            &format!(
                "{}  {}{SEPARATOR}{activity}  {trailing}",
                agent.agent_id, agent.id
            ),
            content_width,
        );
        return Line::from(vec![
            Span::styled(connector, Theme::dim().patch(row_style)),
            Span::styled(detail, Theme::dim().patch(row_style)),
        ]);
    }

    let activity_width = content_width.saturating_sub(fixed_width);
    let activity = truncate_one_line(activity, activity_width);
    let gap = " ".repeat(content_width.saturating_sub(
        identity_width + separator_width + display_width(&activity) + display_width(&trailing),
    ));
    Line::from(vec![
        Span::styled(connector, Theme::dim().patch(row_style)),
        Span::styled(
            agent.agent_id.clone(),
            Theme::text_strong().patch(row_style),
        ),
        Span::styled("  ", row_style),
        Span::styled(agent.id.clone(), Theme::dim().patch(row_style)),
        Span::styled(SEPARATOR, Theme::dim().patch(row_style)),
        Span::styled(activity, Theme::text().patch(row_style)),
        Span::styled(gap, row_style),
        Span::styled(trailing, Theme::dim().patch(row_style)),
    ])
}

fn activity_label(activity: Option<&str>) -> &str {
    match activity {
        Some("assistant text") => "responding",
        Some(activity) => activity.strip_prefix("tool: ").unwrap_or(activity),
        None => "working",
    }
}

fn format_elapsed(seconds: u64) -> String {
    subagent::format_elapsed_secs(seconds)
}
