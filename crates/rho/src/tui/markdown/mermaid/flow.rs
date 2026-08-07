use crate::tui::markdown::mermaid::model::Graph;
use crate::tui::markdown::mermaid::painter::{MermaidArt, MermaidStyles, Oversize};
use crate::tui::terminal_graph;
use unicode_width::UnicodeWidthStr;

mod class;
mod groups;

pub(super) fn render_class(
    graph: &Graph,
    infos: &[super::model::ClassInfo],
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    class::render_class(graph, infos, styles, max_width)
}

const MIN_FLOW_WRAP_WIDTH: usize = 12;
const FLOW_WRAP_STEP: usize = 4;

pub(super) fn layout_flow(
    graph: &Graph,
    styles: &MermaidStyles,
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

    for wrap_width in (MIN_FLOW_WRAP_WIDTH..=terminal_graph::WRAP_WIDTH)
        .rev()
        .step_by(FLOW_WRAP_STEP)
    {
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
            Err(error) => return Err(error),
        }
    }
    Err(Oversize::Width)
}

pub(super) use terminal_graph::Placed;
