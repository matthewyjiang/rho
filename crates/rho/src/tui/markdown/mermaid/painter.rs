// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use ratatui::style::Style;
use ratatui::text::Line;

pub(super) struct MermaidStyles {
    pub(super) border: Style,
    pub(super) node_text: Style,
    pub(super) edge: Style,
    pub(super) edge_label: Style,
}

pub(super) struct MermaidArt {
    pub(super) styled_lines: Vec<Line<'static>>,
    pub(super) plain_lines: Vec<String>,
}

pub(super) const MAX_LABEL: usize = 28;
pub(super) const PAD: usize = 1;
pub(super) const GAP_X: usize = 3;
pub(super) const GAP_Y: usize = 2;
pub(super) const WRAP_WIDTH: usize = 24;
pub(super) const MAX_LINES: usize = 256;
pub(super) const LABEL_BREAK_CHARS: [char; 4] = ['_', '-', '.', '/'];
pub(super) const CONT: char = '\u{0}';
pub(super) const MAX_CANVAS_CELLS: usize = 2_000_000;

pub(super) fn char_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

#[derive(Clone, Copy)]
pub(super) enum Oversize {
    Width,
    Cells,
}
