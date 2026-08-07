use unicode_width::UnicodeWidthStr;

use crate::tui::{
    markdown::mermaid::{model::Graph, MermaidArt},
    terminal_graph::{self, GraphStyles, Oversize},
};

mod class;
mod groups;

pub(super) fn render_class(
    graph: &Graph,
    infos: &[super::model::ClassInfo],
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    class::render_class(graph, infos, styles, max_width)
}

pub(super) fn layout_flow(
    graph: &Graph,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    if graph.groups.is_empty() {
        let art = terminal_graph::layout_flow(&graph.layout_graph(), styles, max_width)?;
        return Ok(MermaidArt {
            styled_lines: art.lines,
            plain_lines: art.plain_lines,
        });
    }

    let layout_graph = graph.layout_graph();

    for wrap_width in terminal_graph::flow_wrap_widths() {
        if !terminal_graph::flow_labels_fit(&layout_graph, wrap_width)
            || graph
                .groups
                .iter()
                .any(|group| group.label.width() > wrap_width)
        {
            continue;
        }
        match groups::render_grouped(graph, styles, max_width, wrap_width) {
            Ok(art) => return Ok(art),
            Err(Oversize::Width) => continue,
            Err(Oversize::Cells) => return Err(Oversize::Cells),
        }
    }
    Err(Oversize::Width)
}
