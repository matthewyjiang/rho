use crate::tui::{
    markdown::mermaid::{
        model::{ClassInfo, Graph},
        MermaidArt,
    },
    terminal_graph::{
        self, Compartment, GraphStyles, NodeExtra, Oversize, TextAlignment, WRAP_WIDTH,
    },
};

pub(super) fn render_class(
    graph: &Graph,
    infos: &[ClassInfo],
    styles: &GraphStyles,
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
            NodeExtra::Compartments(vec![
                Compartment {
                    lines: title,
                    alignment: TextAlignment::Center,
                },
                Compartment {
                    lines: info.attrs.clone(),
                    alignment: TextAlignment::Left,
                },
                Compartment {
                    lines: info.methods.clone(),
                    alignment: TextAlignment::Left,
                },
            ])
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
