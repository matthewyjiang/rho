use ratatui::text::{Line, Span};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
};
use crate::{subagent::RunState, tools::agent::SubagentManager};

const MAX_VISIBLE_AGENTS: usize = 2;
const MAX_AGENT_CONTENT_WIDTH: usize = 52;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunningSubagent {
    id: String,
    preset: String,
    state: RunState,
    last_activity: Option<String>,
    elapsed_seconds: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SubagentPanel {
    agents: Vec<RunningSubagent>,
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
                preset: snapshot.preset,
                state: snapshot.status.state,
                last_activity: snapshot.status.last_activity,
                elapsed_seconds: snapshot.elapsed.as_secs(),
            })
            .collect();
        if self.agents == agents {
            return false;
        }
        self.agents = agents;
        true
    }

    pub(super) fn desired_height(&self) -> usize {
        usize::from(!self.agents.is_empty()) + self.agents.len().min(MAX_VISIBLE_AGENTS)
    }

    pub(super) fn lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        if self.agents.is_empty() || width == 0 || height == 0 {
            return Vec::new();
        }

        let hidden = self.agents.len().saturating_sub(MAX_VISIBLE_AGENTS);
        let noun = if self.agents.len() == 1 {
            "subagent"
        } else {
            "subagents"
        };
        let mut header = vec![
            Span::styled("● ", Theme::success()),
            Span::styled(
                format!("{} {noun} running", self.agents.len()),
                Theme::text_strong(),
            ),
        ];
        if hidden > 0 {
            header.push(Span::styled(format!("  +{hidden} more"), Theme::dim()));
        }
        let mut lines = vec![Line::from(header)];

        if height == 1 {
            return lines;
        }
        let visible_count = self
            .agents
            .len()
            .min(MAX_VISIBLE_AGENTS)
            .min(height.saturating_sub(1));
        for (index, agent) in self.agents.iter().take(visible_count).enumerate() {
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
            lines.push(agent_line(agent, activity, connector, width));
        }
        lines
    }
}

fn agent_line(
    agent: &RunningSubagent,
    activity: &str,
    connector: &'static str,
    width: usize,
) -> Line<'static> {
    const SEPARATOR: &str = "  ·  ";
    const MIN_GAP: usize = 2;

    let connector_width = display_width(connector);
    let content_width = width
        .saturating_sub(connector_width)
        .min(MAX_AGENT_CONTENT_WIDTH);
    let identity_width = display_width(&agent.preset) + 2 + display_width(&agent.id);
    let separator_width = display_width(SEPARATOR);
    let elapsed = format_elapsed(agent.elapsed_seconds);
    let fixed_width = identity_width + separator_width + MIN_GAP + display_width(&elapsed);

    if fixed_width >= content_width {
        let detail = truncate_one_line(
            &format!(
                "{}  {}{SEPARATOR}{activity}  {elapsed}",
                agent.preset, agent.id
            ),
            content_width,
        );
        return Line::from(vec![
            Span::styled(connector, Theme::dim()),
            Span::styled(detail, Theme::dim()),
        ]);
    }

    let activity_width = content_width.saturating_sub(fixed_width);
    let activity = truncate_one_line(activity, activity_width);
    let gap = " ".repeat(content_width.saturating_sub(
        identity_width + separator_width + display_width(&activity) + display_width(&elapsed),
    ));
    Line::from(vec![
        Span::styled(connector, Theme::dim()),
        Span::styled(agent.preset.clone(), Theme::text_strong()),
        Span::raw("  "),
        Span::styled(agent.id.clone(), Theme::dim()),
        Span::styled(SEPARATOR, Theme::dim()),
        Span::styled(activity, Theme::text()),
        Span::raw(gap),
        Span::styled(elapsed, Theme::dim()),
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
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes:02}m")
}

#[cfg(test)]
#[path = "subagent_panel_tests.rs"]
mod tests;
