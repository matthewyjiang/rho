// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use ratatui::style::Style;
use ratatui::text::Line;

use super::{Node, NodeRect, NodeStyle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct GraphStyles {
    pub(in crate::tui) border: Style,
    pub(in crate::tui) node_text: Style,
    pub(in crate::tui) edge: Style,
    pub(in crate::tui) edge_label: Style,
    pub(in crate::tui) node_styles: Vec<NodeStyle>,
}

impl GraphStyles {
    pub(in crate::tui) fn for_nodes(nodes: &[Node], edge: Style) -> Self {
        let fallback = nodes.first().map(|node| node.style).unwrap_or_default();
        Self {
            border: fallback.border,
            node_text: fallback.text,
            edge,
            edge_label: edge,
            node_styles: nodes.iter().map(|node| node.style).collect(),
        }
    }

    pub(in crate::tui) fn node_style(&self, index: usize) -> NodeStyle {
        self.node_styles
            .get(index)
            .copied()
            .unwrap_or_else(|| NodeStyle::new(self.border, self.node_text))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct GraphArt {
    pub(in crate::tui) lines: Vec<Line<'static>>,
    pub(in crate::tui) plain_lines: Vec<String>,
    pub(in crate::tui) width: usize,
    pub(in crate::tui) height: usize,
    pub(in crate::tui) node_rects: Vec<NodeRect>,
}

pub(in crate::tui) const MAX_LABEL: usize = 28;
pub(in crate::tui) const PAD: usize = 1;
pub(in crate::tui) const GAP_X: usize = 3;
pub(in crate::tui) const GAP_Y: usize = 2;
pub(in crate::tui) const WRAP_WIDTH: usize = 24;
pub(in crate::tui) const MAX_LINES: usize = 256;
pub(in crate::tui) const LABEL_BREAK_CHARS: [char; 4] = ['_', '-', '.', '/'];
pub(in crate::tui) const CONT: char = '\u{0}';
pub(in crate::tui) const MAX_CANVAS_CELLS: usize = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Oversize {
    Width,
    Cells,
}
