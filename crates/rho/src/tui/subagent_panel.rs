use ratatui::text::{Line, Span};

use super::{render::truncate_one_line, theme::Theme};
use crate::{subagent::RunState, tools::agent::SubagentManager};

const MAX_VISIBLE_AGENTS: usize = 2;

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
        for agent in self
            .agents
            .iter()
            .take(MAX_VISIBLE_AGENTS)
            .take(height.saturating_sub(1))
        {
            let state = match agent.state {
                RunState::Starting => "starting",
                RunState::Running => agent.last_activity.as_deref().unwrap_or("working"),
                RunState::Ok | RunState::Error | RunState::Stopped => continue,
            };
            let available = width.saturating_sub(4);
            let detail = truncate_one_line(
                &format!(
                    "{}  {}  ·  {}  ·  {}",
                    agent.preset,
                    agent.id,
                    state,
                    format_elapsed(agent.elapsed_seconds)
                ),
                available,
            );
            lines.push(Line::from(vec![
                Span::styled("  └ ", Theme::dim()),
                Span::styled(detail, Theme::dim()),
            ]));
        }
        lines
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
