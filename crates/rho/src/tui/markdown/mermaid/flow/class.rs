// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use unicode_width::UnicodeWidthStr;

use super::{layout_canvas, NodeExtra, Placed};
use crate::tui::markdown::mermaid::{
    canvas::{Canvas, Cls},
    drawing::{draw_box, draw_seq_text, fit_label},
    model::{ClassInfo, Dir, Graph, Shape},
    painter::{MermaidArt, MermaidStyles, Oversize, PAD},
};

pub(crate) fn render_class(
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
            for a in &info.annotations {
                title.push(format!("«{a}»"));
            }
            title.push(node.label.clone());
            NodeExtra::Compartments(vec![title, info.attrs.clone(), info.methods.clone()])
        })
        .collect();
    let mut canvas = layout_canvas(graph, &extras, max_width)?;
    match graph.dir {
        Dir::Up => canvas.flip_vertical(),
        Dir::Left => canvas.flip_horizontal(),
        _ => {}
    }
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
    })
}

pub(super) fn draw_class_box(canvas: &mut Canvas, p: &Placed, sections: &[Vec<String>]) {
    draw_box(canvas, p, &[], Shape::Rect);
    let inner = p.w.saturating_sub(2 * PAD + 2).max(1);
    let mut row = p.y + 1;
    let mut first = true;
    for (si, section) in sections.iter().enumerate() {
        if section.is_empty() {
            continue;
        }
        if !first {
            canvas.set(p.x, row, '├', Cls::Border);
            for x in (p.x + 1)..(p.x + p.w - 1) {
                canvas.set(x, row, '─', Cls::Border);
            }
            canvas.set(p.x + p.w - 1, row, '┤', Cls::Border);
            row += 1;
        }
        first = false;
        for line in section {
            let text = fit_label(line, inner);
            let tx = if si == 0 {
                p.x + 1 + PAD + inner.saturating_sub(text.width()) / 2
            } else {
                p.x + 1 + PAD
            };
            draw_seq_text(canvas, &text, tx, row, Cls::Text);
            row += 1;
        }
    }
}
