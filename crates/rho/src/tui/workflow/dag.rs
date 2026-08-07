//! Workflow policy for the shared terminal graph renderer.

use std::collections::BTreeMap;

use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use crate::{
    tui::{
        render::truncate_one_line,
        terminal_graph::{
            Edge, Graph, GraphArt, Node, NodeRect, NodeStyle, Oversize, RankOrdering,
        },
        theme::Theme,
    },
    workflow::{NodeId, NodeState, NodeTerminalState},
};

use super::event_adapter::WorkflowNodeSnapshot;

// Preserve the former chip's activity budget so untrusted progress text cannot
// force the whole graph past the renderer's line limit.
const MAX_GRAPH_ACTIVITY_WIDTH: usize = 28;

#[derive(Clone, Copy)]
pub(super) enum HorizontalDirection {
    Left,
    Right,
}

pub(super) struct DagRender {
    pub(super) lines: Vec<Line<'static>>,
    canvas_width: usize,
    canvas_height: usize,
    node_rects: Vec<NodeRect>,
}

impl DagRender {
    fn from_art(art: GraphArt) -> Self {
        Self {
            lines: art.lines,
            canvas_width: art.width,
            canvas_height: art.height,
            node_rects: art.node_rects,
        }
    }

    fn message(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            canvas_width: unicode_width::UnicodeWidthStr::width(message.as_str()),
            canvas_height: 1,
            lines: vec![Line::from(message)],
            node_rects: Vec::new(),
        }
    }

    pub(super) fn viewport_offset(&self, selected: usize, width: u16, height: u16) -> (u16, u16) {
        let Some(rect) = self.node_rects.get(selected).copied() else {
            return (0, 0);
        };
        let x = follow_axis(rect.x, rect.width, self.canvas_width, usize::from(width));
        let y = follow_axis(rect.y, rect.height, self.canvas_height, usize::from(height));
        (to_u16(y), to_u16(x))
    }
}

pub(super) fn node_ranks(nodes: &[WorkflowNodeSnapshot]) -> Vec<usize> {
    workflow_graph(nodes, /*selected*/ 0, &[]).ranks()
}

pub(super) fn horizontal_neighbor(
    ranks: &[usize],
    selected: usize,
    direction: HorizontalDirection,
) -> Option<usize> {
    let selected_rank = *ranks.get(selected)?;
    match direction {
        HorizontalDirection::Left => (0..selected)
            .rev()
            .find(|&index| ranks[index] == selected_rank),
        HorizontalDirection::Right => {
            ((selected + 1)..ranks.len()).find(|&index| ranks[index] == selected_rank)
        }
    }
}

/// Render the complete workflow DAG. The view clips this canvas and follows
/// the selected node instead of asking the graph layout to discard topology.
pub(super) fn render_dag(
    nodes: &[WorkflowNodeSnapshot],
    selected: usize,
    live_activity: &[Option<String>],
) -> DagRender {
    if nodes.is_empty() {
        return DagRender::message("no steps");
    }

    let graph = workflow_graph(nodes, selected, live_activity);
    match graph.render(Theme::dim()) {
        Ok(art) => DagRender::from_art(art),
        Err(Oversize::Width | Oversize::Cells) => {
            DagRender::message("graph exceeds the terminal render budget")
        }
    }
}

fn workflow_graph(
    nodes: &[WorkflowNodeSnapshot],
    selected: usize,
    live_activity: &[Option<String>],
) -> Graph {
    let index_by_id = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect::<BTreeMap<NodeId, usize>>();
    let graph_nodes = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let activity = live_activity
                .get(index)
                .and_then(|value| value.as_deref())
                .or_else(|| running_activity(node));
            let label = match activity {
                Some(activity) => {
                    let activity = truncate_one_line(activity, MAX_GRAPH_ACTIVITY_WIDTH);
                    format!(
                        "{} {} · {activity}",
                        state_glyph(&node.state),
                        node.display_name
                    )
                }
                None => format!("{} {}", state_glyph(&node.state), node.display_name),
            };
            Node::rectangular(label, node_style(&node.state, index == selected))
        })
        .collect();
    let edges = nodes
        .iter()
        .enumerate()
        .flat_map(|(to, node)| {
            let index_by_id = &index_by_id;
            node.dependencies.iter().map(move |dependency| {
                let from = index_by_id
                    .get(dependency)
                    .copied()
                    .expect("workflow snapshot dependencies refer to frozen nodes");
                Edge::directed(from, to)
            })
        })
        .collect();

    Graph::top_down(graph_nodes, edges, RankOrdering::PreserveInput)
        .expect("workflow graph maps dependency ids to valid node indices")
}

fn node_style(state: &NodeState, selected: bool) -> NodeStyle {
    let state = state_style(state);
    if selected {
        NodeStyle::new(
            state.add_modifier(Modifier::BOLD),
            state.add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
    } else {
        NodeStyle::uniform(state)
    }
}

fn running_activity(node: &WorkflowNodeSnapshot) -> Option<&str> {
    matches!(node.state, NodeState::Running { .. }).then_some(if node.work.is_empty() {
        "working"
    } else {
        node.work.as_str()
    })
}

fn follow_axis(start: usize, length: usize, canvas: usize, viewport: usize) -> usize {
    if viewport == 0 || canvas <= viewport {
        return 0;
    }
    if length > viewport {
        return start.min(canvas.saturating_sub(viewport));
    }
    let node_center = start.saturating_add(length / 2);
    node_center
        .saturating_sub(viewport / 2)
        .min(canvas.saturating_sub(viewport))
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(super) fn state_glyph(state: &NodeState) -> &'static str {
    match state {
        NodeState::Pending => "·",
        NodeState::Ready => "○",
        NodeState::Running { .. } => "●",
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => "✓",
            NodeTerminalState::Skipped => "–",
            NodeTerminalState::Failure
            | NodeTerminalState::Denial
            | NodeTerminalState::Cancellation
            | NodeTerminalState::Blocked => "✗",
        },
    }
}

pub(super) fn state_style(state: &NodeState) -> Style {
    match state {
        NodeState::Pending => Theme::dim(),
        NodeState::Ready => Theme::text(),
        NodeState::Running { .. } => Theme::accent(),
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => Theme::success(),
            NodeTerminalState::Skipped => Theme::warning(),
            NodeTerminalState::Failure
            | NodeTerminalState::Denial
            | NodeTerminalState::Cancellation
            | NodeTerminalState::Blocked => Theme::error(),
        },
    }
}

pub(super) fn state_label(state: &NodeState) -> String {
    match state {
        NodeState::Pending => "waiting".into(),
        NodeState::Ready => "ready".into(),
        NodeState::Running { attempt } => format!("running · try {attempt}"),
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => "done".into(),
            NodeTerminalState::Failure => "failed".into(),
            NodeTerminalState::Denial => "denied".into(),
            NodeTerminalState::Cancellation => "cancelled".into(),
            NodeTerminalState::Skipped => "skipped".into(),
            NodeTerminalState::Blocked => "blocked".into(),
        },
    }
}

#[cfg(test)]
#[path = "dag_tests.rs"]
mod tests;
