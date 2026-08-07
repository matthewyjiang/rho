use ratatui::text::Line;

pub(super) use crate::tui::terminal_graph::{
    GraphStyles as MermaidStyles, Oversize, MAX_CANVAS_CELLS, MAX_LABEL, MAX_LINES, PAD, WRAP_WIDTH,
};

pub(super) struct MermaidArt {
    pub(super) styled_lines: Vec<Line<'static>>,
    pub(super) plain_lines: Vec<String>,
}
