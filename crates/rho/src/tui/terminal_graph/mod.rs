//! Generic terminal graph layout and painting.
//!
//! Mermaid adapts its parsed model to this renderer, while the workflow TUI can
//! build a graph directly without producing Mermaid source or depending on its
//! parser and fallback policy.

use ratatui::style::Style;

mod canvas;
mod drawing;
mod flow;
mod ordering;
mod painter;
mod placement;

pub(in crate::tui) use canvas::{Canvas, Cls as CellClass, D, L, R, U};
pub(in crate::tui) use drawing::{draw_box, draw_seq_text, fit_label, wrap_label};
pub(in crate::tui) use flow::{
    art_from_layout, flow_labels_fit, layout_canvas, layout_flow, NodeExtra, Placed,
};
pub(in crate::tui) use painter::{
    GraphArt, GraphStyles, Oversize, MAX_CANVAS_CELLS, MAX_LABEL, MAX_LINES, PAD, WRAP_WIDTH,
};

/// A node's independent border and text styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct NodeStyle {
    pub(in crate::tui) border: Style,
    pub(in crate::tui) text: Style,
}

impl NodeStyle {
    pub(in crate::tui) const fn new(border: Style, text: Style) -> Self {
        Self { border, text }
    }

    pub(in crate::tui) const fn uniform(style: Style) -> Self {
        Self {
            border: style,
            text: style,
        }
    }
}

/// Shapes supported by the neutral painter. The workflow API uses rectangles;
/// Mermaid keeps the other two shapes for its existing flowchart behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum NodeShape {
    Rect,
    Round,
    Diamond,
}

/// The direction used by the generic rank layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Direction {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

/// Endpoint decorations used by Mermaid and preserved by the shared router.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum EdgeHead {
    None,
    Arrow,
    Circle,
    Cross,
    Triangle,
    DiamondFill,
    DiamondOpen,
}

/// Line glyph treatment. Color and terminal style come from the global edge
/// style; this enum only selects solid, dotted, or thick route glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum EdgeLine {
    Solid,
    Dotted,
    Thick,
}

/// An ordered graph node. The vector order is retained by layout and by the
/// node rectangles in [`GraphArt`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Node {
    pub(in crate::tui) label: String,
    pub(in crate::tui) shape: NodeShape,
    pub(in crate::tui) style: NodeStyle,
}

impl Node {
    pub(in crate::tui) fn rectangular(label: impl Into<String>, style: NodeStyle) -> Self {
        Self {
            label: label.into(),
            shape: NodeShape::Rect,
            style,
        }
    }
}

/// An explicit directed edge from `from` to `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Edge {
    pub(in crate::tui) from: usize,
    pub(in crate::tui) to: usize,
    pub(in crate::tui) label: Option<String>,
    pub(in crate::tui) head_to: EdgeHead,
    pub(in crate::tui) head_from: EdgeHead,
    pub(in crate::tui) line: EdgeLine,
}

impl Edge {
    pub(in crate::tui) fn directed(from: usize, to: usize) -> Self {
        Self {
            from,
            to,
            label: None,
            head_to: EdgeHead::Arrow,
            head_from: EdgeHead::None,
            line: EdgeLine::Solid,
        }
    }
}

/// Errors found before or during graph layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum GraphError {
    InvalidEdgeEndpoint,
}

/// A graph with stable node order and explicit directed edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Graph {
    pub(in crate::tui) nodes: Vec<Node>,
    pub(in crate::tui) edges: Vec<Edge>,
    pub(in crate::tui) direction: Direction,
}

impl Graph {
    /// Build the workflow-facing top-down graph.
    pub(in crate::tui) fn top_down(nodes: Vec<Node>, edges: Vec<Edge>) -> Result<Self, GraphError> {
        Self::from_parts(nodes, edges, Direction::TopDown)
    }

    /// Build a graph for an existing terminal diagram adapter.
    pub(in crate::tui) fn from_parts(
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        direction: Direction,
    ) -> Result<Self, GraphError> {
        if edges
            .iter()
            .any(|edge| edge.from >= nodes.len() || edge.to >= nodes.len())
        {
            return Err(GraphError::InvalidEdgeEndpoint);
        }
        Ok(Self {
            nodes,
            edges,
            direction,
        })
    }

    /// Render the complete graph without clipping it to a viewport width.
    /// Callers can clip the returned lines and use `node_rects` to follow a
    /// selected node while retaining the full canvas dimensions.
    pub(in crate::tui) fn render(&self, edge_style: Style) -> Result<GraphArt, Oversize> {
        let styles = GraphStyles::for_nodes(&self.nodes, edge_style);
        layout_flow(self, &styles, /*max_width*/ None)
    }
}

/// Geometry for one node in the rendered canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct NodeRect {
    pub(in crate::tui) x: usize,
    pub(in crate::tui) y: usize,
    pub(in crate::tui) width: usize,
    pub(in crate::tui) height: usize,
}

#[cfg(test)]
#[path = "terminal_graph_tests.rs"]
mod tests;
