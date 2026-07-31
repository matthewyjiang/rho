//! Compact layered DAG layout for the workflow run screen.

use std::collections::{BTreeMap, BTreeSet};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::workflow::{NodeId, NodeState, NodeTerminalState};

use super::event_adapter::WorkflowNodeSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DagLine {
    pub(super) spans: Vec<Span<'static>>,
    /// Node this line selects when the cursor is on it.
    pub(super) node_index: Option<usize>,
}

/// Build top-to-bottom layered DAG lines with selection highlight.
pub(super) fn render_dag(
    nodes: &[WorkflowNodeSnapshot],
    selected: usize,
    width: u16,
) -> Vec<DagLine> {
    if nodes.is_empty() {
        return vec![DagLine {
            spans: vec![Span::raw("no steps")],
            node_index: None,
        }];
    }

    let width = width.max(12) as usize;
    let index_by_id = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect::<BTreeMap<_, _>>();

    let layers = topological_layers(nodes, &index_by_id);
    let mut lines = Vec::new();

    for (layer_idx, layer) in layers.iter().enumerate() {
        if layer_idx > 0 {
            lines.push(connector_line(layer, nodes, width));
        }
        lines.extend(layer_lines(layer, nodes, selected, width));
    }
    lines
}

fn topological_layers(
    nodes: &[WorkflowNodeSnapshot],
    index_by_id: &BTreeMap<NodeId, usize>,
) -> Vec<Vec<usize>> {
    let mut depth = vec![0usize; nodes.len()];
    // Longest-path depth so dependents sit below every parent.
    let mut changed = true;
    let mut guard = 0;
    while changed && guard < nodes.len() + 2 {
        changed = false;
        guard += 1;
        for (index, node) in nodes.iter().enumerate() {
            let parent_depth = node
                .dependencies
                .iter()
                .filter_map(|dep| index_by_id.get(dep).copied())
                .map(|parent| depth[parent])
                .max()
                .map(|value| value + 1)
                .unwrap_or(0);
            if parent_depth > depth[index] {
                depth[index] = parent_depth;
                changed = true;
            }
        }
    }

    let max_depth = depth.iter().copied().max().unwrap_or(0);
    let mut layers = vec![Vec::new(); max_depth + 1];
    let mut order = (0..nodes.len()).collect::<Vec<_>>();
    // Stable left-to-right: original snapshot order within a layer.
    order.sort_by_key(|&index| (depth[index], index));
    for index in order {
        layers[depth[index]].push(index);
    }
    layers
}

fn layer_lines(
    layer: &[usize],
    nodes: &[WorkflowNodeSnapshot],
    selected: usize,
    width: usize,
) -> Vec<DagLine> {
    // Prefer one row when labels fit; otherwise stack nodes in the layer.
    let labels = layer
        .iter()
        .map(|&index| node_chip(nodes, index, index == selected))
        .collect::<Vec<_>>();
    let joined_width = labels.iter().map(|chip| chip.display_width).sum::<usize>()
        + labels.len().saturating_sub(1) * 3;

    if joined_width <= width && labels.len() > 1 {
        let mut spans = Vec::new();
        for (offset, chip) in labels.into_iter().enumerate() {
            if offset > 0 {
                spans.push(Span::raw("   "));
            }
            spans.extend(chip.spans);
        }
        // Multi-node row: keep selection on the selected node if present.
        let node_index = layer.iter().copied().find(|&index| index == selected);
        return vec![DagLine { spans, node_index }];
    }

    layer
        .iter()
        .map(|&index| {
            let chip = node_chip(nodes, index, index == selected);
            DagLine {
                spans: chip.spans,
                node_index: Some(index),
            }
        })
        .collect()
}

fn connector_line(layer: &[usize], nodes: &[WorkflowNodeSnapshot], width: usize) -> DagLine {
    // Show which parents feed this layer, without repeating full topology noise.
    let parents = layer
        .iter()
        .flat_map(|&index| nodes[index].dependencies.iter())
        .collect::<BTreeSet<_>>();
    let label = if parents.len() <= 1 {
        "│".into()
    } else {
        format!("│  ({} inputs)", parents.len())
    };
    let truncated = truncate(&label, width);
    DagLine {
        spans: vec![Span::styled(
            truncated,
            Style::default().fg(Color::DarkGray),
        )],
        node_index: None,
    }
}

struct Chip {
    spans: Vec<Span<'static>>,
    display_width: usize,
}

fn node_chip(nodes: &[WorkflowNodeSnapshot], index: usize, selected: bool) -> Chip {
    let node = &nodes[index];
    let glyph = state_glyph(&node.state);
    let name = truncate(&node.display_name, 28);
    let marker = if selected { "▶ " } else { "  " };
    let body = format!("{marker}{glyph} {name}");
    let style = if selected {
        state_style(&node.state).add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        state_style(&node.state)
    };
    let display_width = UnicodeWidthStr::width(body.as_str());
    Chip {
        spans: vec![Span::styled(body, style)],
        display_width,
    }
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
    let color = match state {
        NodeState::Pending => Color::DarkGray,
        NodeState::Ready => Color::Gray,
        NodeState::Running { .. } => Color::Cyan,
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => Color::Green,
            NodeTerminalState::Skipped => Color::Yellow,
            NodeTerminalState::Failure
            | NodeTerminalState::Denial
            | NodeTerminalState::Cancellation
            | NodeTerminalState::Blocked => Color::Red,
        },
    };
    Style::default().fg(color)
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

fn truncate(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    if max_width <= 1 {
        return "…".into();
    }
    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]));
        if width + ch_width > max_width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

/// Ensure layered depths place dependents below dependencies.
#[cfg(test)]
pub(super) fn layer_index_of(nodes: &[WorkflowNodeSnapshot], node_index: usize) -> Option<usize> {
    let index_by_id = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let layers = topological_layers(nodes, &index_by_id);
    layers.iter().position(|layer| layer.contains(&node_index))
}

pub(super) fn to_paragraph_lines(lines: Vec<DagLine>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| Line::from(line.spans))
        .collect()
}

#[cfg(test)]
#[path = "dag_tests.rs"]
mod tests;
