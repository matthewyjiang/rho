use crate::tui::markdown::mermaid::{
    model::{ClassInfo, Graph},
    painter::{MermaidArt, MermaidStyles, Oversize, WRAP_WIDTH},
};
use crate::tui::terminal_graph::{self, NodeExtra};

pub(super) fn render_class(
    graph: &Graph,
    infos: &[ClassInfo],
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let extras: Vec<NodeExtra> = graph
        .nodes
        .iter()
        .zip(infos)
        .map(|(node, info)| {
            let mut title = Vec::new();
            for annotation in &info.annotations {
                title.push(format!("«{annotation}»"));
            }
            title.push(node.label.clone());
            NodeExtra::Compartments(vec![title, info.attrs.clone(), info.methods.clone()])
        })
        .collect();
    let graph = graph.layout_graph();
    let layout = terminal_graph::layout_canvas(&graph, &extras, max_width, WRAP_WIDTH)?;
    let art = terminal_graph::art_from_layout(&graph, layout, styles);
    Ok(MermaidArt {
        styled_lines: art.lines,
        plain_lines: art.plain_lines,
    })
}
