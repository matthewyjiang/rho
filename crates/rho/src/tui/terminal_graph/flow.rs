// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use unicode_width::UnicodeWidthStr;

use super::{
    canvas::{Canvas, STY_DOT, STY_SOLID, STY_THICK},
    drawing::{
        art_node_rect, compute_ranks, draw_box, draw_compartment_box, draw_frame, route_back,
        route_back_lr, route_forward, route_forward_lr, route_self, route_skip, wrap_label,
        SkipPath,
    },
    ordering::order_ranks,
    painter::{
        GraphArt, GraphStyles, Oversize, EDGE_LABEL_MAX_LINES, MAX_CANVAS_CELLS, MAX_LABEL,
        MAX_LINES, PAD, WRAP_WIDTH,
    },
    placement::{place_lr, place_td},
    Compartment, Direction, EdgeLine, Graph, RankOrdering,
};

const MIN_FLOW_WRAP_WIDTH: usize = 12;
const FLOW_WRAP_STEP: usize = 4;

// Keep the established width required by the self-loop's two endpoint cells
// and route padding.
const MIN_SELF_LOOP_WIDTH: usize = 7;

fn flow_wrap_widths() -> impl Iterator<Item = usize> {
    (MIN_FLOW_WRAP_WIDTH..=WRAP_WIDTH)
        .rev()
        .step_by(FLOW_WRAP_STEP)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Placed {
    pub(in crate::tui) x: usize,
    pub(in crate::tui) y: usize,
    pub(in crate::tui) w: usize,
    pub(in crate::tui) h: usize,
    pub(in crate::tui) cx: usize,
    pub(in crate::tui) cy: usize,
    pub(in crate::tui) rank: usize,
}

pub(super) struct NodeSizes {
    pub(super) box_w: Vec<usize>,
    pub(super) box_h: Vec<usize>,
    pub(super) lay_w: Vec<usize>,
    pub(super) lay_h: Vec<usize>,
    pub(super) extra_h: Vec<usize>,
    pub(super) self_label_w: Vec<usize>,
}

/// Intermediate layout output. The canvas is full-size and the placement
/// vector stays in stable input-node order for clipping and follow behavior.
pub(in crate::tui) struct LayoutCanvas {
    pub(in crate::tui) canvas: Canvas,
    pub(in crate::tui) placed: Vec<Placed>,
}

/// Walk wrap-width rungs from wide to tight, skipping any width that cannot
/// hold node labels, until a layout succeeds.
pub(in crate::tui) fn over_wrap_rungs<T>(
    graph: &Graph,
    mut try_rung: impl FnMut(usize) -> Result<T, Oversize>,
) -> Result<T, Oversize> {
    for wrap_width in flow_wrap_widths() {
        if !flow_labels_fit(graph, wrap_width) {
            continue;
        }
        match try_rung(wrap_width) {
            Ok(value) => return Ok(value),
            Err(Oversize::Width) => continue,
            Err(Oversize::Cells) => return Err(Oversize::Cells),
        }
    }
    Err(Oversize::Width)
}

/// Lays out a plain topological graph, retrying at tighter label wraps when
/// the requested maximum width cannot hold the first layout.
pub(in crate::tui) fn layout_flow(
    graph: &Graph,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<GraphArt, Oversize> {
    over_wrap_rungs(graph, |wrap_width| {
        layout_plain_flow(graph, styles, max_width, wrap_width)
    })
}

fn layout_plain_flow(
    graph: &Graph,
    styles: &GraphStyles,
    max_width: Option<usize>,
    wrap_width: usize,
) -> Result<GraphArt, Oversize> {
    let extras: Vec<NodeExtra> = (0..graph.nodes.len()).map(|_| NodeExtra::Plain).collect();
    let layout = layout_canvas(graph, &extras, max_width, wrap_width)?;
    Ok(art_from_layout(graph, layout, styles))
}

/// Compaction must never drop label text, so a wrap width that cannot hold
/// every node label within the painter's line budget is skipped entirely.
fn flow_labels_fit(graph: &Graph, wrap_width: usize) -> bool {
    graph
        .nodes
        .iter()
        .all(|node| wrap_label(&node.label, wrap_width, usize::MAX).len() <= MAX_LINES)
}

pub(in crate::tui) enum NodeExtra {
    Plain,
    Frame(Canvas),
    Compartments(Vec<Compartment>),
}

pub(in crate::tui) fn layout_canvas(
    graph: &Graph,
    extras: &[NodeExtra],
    max_width: Option<usize>,
    wrap_width: usize,
) -> Result<LayoutCanvas, Oversize> {
    let n = graph.nodes.len();
    assert_eq!(
        extras.len(),
        n,
        "node extras must match the validated graph node count"
    );
    if n == 0 {
        return Ok(LayoutCanvas {
            canvas: Canvas::new(0, 0),
            placed: Vec::new(),
        });
    }

    let ranks = compute_ranks(graph);
    let max_rank = *ranks.iter().max().unwrap_or(&0);

    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    for (idx, &r) in ranks.iter().enumerate() {
        by_rank[r].push(idx);
    }
    match graph.rank_ordering {
        RankOrdering::PreserveInput => {}
        RankOrdering::MinimizeCrossings => order_ranks(&mut by_rank, &graph.edges, &ranks),
    }

    let wrapped: Vec<Vec<String>> = graph
        .nodes
        .iter()
        .map(|node| wrap_label(&node.label, wrap_width, MAX_LINES))
        .collect();
    // Edge labels compact with the same ladder rung as node labels so the
    // retry loop shrinks label corridors too, not just boxes. Self-loop labels
    // stay single-line beside their loop.
    let edge_labels: Vec<Vec<String>> = graph
        .edges
        .iter()
        .map(|edge| match &edge.label {
            Some(label) if edge.from != edge.to => {
                wrap_label(label, wrap_width, EDGE_LABEL_MAX_LINES)
            }
            _ => Vec::new(),
        })
        .collect();
    let mut box_w: Vec<usize> = (0..n)
        .map(|i| match &extras[i] {
            NodeExtra::Frame(sub) => {
                // Reserve the real title when the pane can hold it. wrap_width
                // is a node-label compaction knob, not a title budget; using it
                // here ellipsized group titles even in a wide pane.
                let title_w = match max_width {
                    Some(max) => graph.nodes[i].label.width().min(max.saturating_sub(4)),
                    None => graph.nodes[i].label.width(),
                };
                (sub.w + 2).max(title_w + 4)
            }
            NodeExtra::Compartments(compartments) => {
                compartments
                    .iter()
                    .flat_map(|compartment| &compartment.lines)
                    .map(|line| line.width())
                    .max()
                    .unwrap_or(1)
                    .max(1)
                    + 2 * PAD
                    + 2
            }
            NodeExtra::Plain => {
                wrapped[i]
                    .iter()
                    .map(|line| line.width())
                    .max()
                    .unwrap_or(1)
                    .max(1)
                    + 2 * PAD
                    + 2
            }
        })
        .collect();
    let box_h: Vec<usize> = (0..n)
        .map(|i| match &extras[i] {
            NodeExtra::Frame(sub) => sub.h + 2,
            NodeExtra::Compartments(compartments) => {
                let filled = compartments
                    .iter()
                    .filter(|compartment| !compartment.lines.is_empty())
                    .count();
                compartments
                    .iter()
                    .map(|compartment| compartment.lines.len())
                    .sum::<usize>()
                    + filled.saturating_sub(1)
                    + 2
            }
            NodeExtra::Plain => wrapped[i].len() + 2,
        })
        .collect();

    let mut extra_h = vec![0usize; n];
    let mut self_label_w = vec![0usize; n];
    for edge in &graph.edges {
        if edge.from == edge.to {
            extra_h[edge.from] = 2;
            if let Some(label) = &edge.label {
                self_label_w[edge.from] = self_label_w[edge.from].max(label.width().min(MAX_LABEL));
            }
        }
    }
    for i in 0..n {
        if extra_h[i] > 0 {
            box_w[i] = box_w[i].max(MIN_SELF_LOOP_WIDTH);
        }
    }
    let lay_w: Vec<usize> = (0..n)
        .map(|i| {
            box_w[i]
                + if self_label_w[i] > 0 {
                    2 * (self_label_w[i] + 3)
                } else {
                    0
                }
        })
        .collect();
    let lay_h: Vec<usize> = (0..n).map(|i| box_h[i] + extra_h[i]).collect();
    let sizes = NodeSizes {
        box_w,
        box_h,
        lay_w,
        lay_h,
        extra_h,
        self_label_w,
    };

    let mut placed = vec![
        Placed {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            cx: 0,
            cy: 0,
            rank: 0,
        };
        n
    ];

    let vertical = matches!(graph.direction, Direction::TopDown | Direction::BottomUp);
    let plan = if vertical {
        place_td(
            &ranks,
            max_rank,
            &by_rank,
            &sizes,
            graph,
            &edge_labels,
            &mut placed,
        )
    } else {
        place_lr(
            &ranks,
            max_rank,
            &by_rank,
            &sizes,
            graph,
            &edge_labels,
            &mut placed,
        )
    };
    let (canvas_w, canvas_h) = plan.canvas;

    if max_width.is_some_and(|width| canvas_w > width) {
        return Err(Oversize::Width);
    }
    if canvas_w.saturating_mul(canvas_h) > MAX_CANVAS_CELLS {
        return Err(Oversize::Cells);
    }

    let mut canvas = Canvas::new(canvas_w, canvas_h);
    for idx in 0..n {
        match &extras[idx] {
            NodeExtra::Frame(sub) => {
                draw_frame(
                    &mut canvas,
                    &placed[idx],
                    &graph.nodes[idx].label,
                    sub,
                    /*node_index*/ Some(idx),
                );
            }
            NodeExtra::Compartments(sections) => {
                draw_compartment_box(
                    &mut canvas,
                    &placed[idx],
                    sections,
                    /*node_index*/ Some(idx),
                );
            }
            NodeExtra::Plain => draw_box(
                &mut canvas,
                &placed[idx],
                &wrapped[idx],
                graph.nodes[idx].shape,
                /*node_index*/ Some(idx),
            ),
        }
    }
    for (i, edge) in graph.edges.iter().enumerate() {
        canvas.cur_style = match edge.line {
            EdgeLine::Solid => STY_SOLID,
            EdgeLine::Dotted => STY_DOT,
            EdgeLine::Thick => STY_THICK,
        };
        if edge.from == edge.to {
            route_self(&mut canvas, &placed[edge.from], edge);
            continue;
        }
        let (from, to) = (&placed[edge.from], &placed[edge.to]);
        let adjacent = to.rank == from.rank + 1;
        let bus = plan.band_end[from.rank] + plan.edge_bus[i];
        let lane = plan.lane_base + plan.edge_lane[i];
        let label_lines = edge_labels[i].as_slice();
        match (vertical, adjacent) {
            (true, true) => route_forward(
                &mut canvas,
                from,
                to,
                edge,
                bus,
                plan.source_anchors[edge.from],
                label_lines,
            ),
            (true, false) if to.rank > from.rank + 1 => route_skip(
                &mut canvas,
                from,
                to,
                edge,
                SkipPath {
                    exit_row: bus,
                    lane_x: lane,
                    join_row: plan.edge_join[i],
                    source_anchor: plan.source_anchors[edge.from],
                },
                label_lines,
            ),
            (true, false) => route_back(&mut canvas, from, to, edge, lane, label_lines),
            (false, true) => route_forward_lr(
                &mut canvas,
                from,
                to,
                edge,
                bus,
                plan.source_anchors[edge.from],
                label_lines,
            ),
            (false, false) => route_back_lr(&mut canvas, from, to, edge, lane, label_lines),
        }
    }

    canvas.finalize_mask();
    Ok(LayoutCanvas { canvas, placed })
}

pub(in crate::tui) fn art_from_layout(
    graph: &Graph,
    mut layout: LayoutCanvas,
    styles: &GraphStyles,
) -> GraphArt {
    let mut rects = layout
        .placed
        .iter()
        .map(|placed| art_node_rect(*placed, layout.canvas.w, layout.canvas.h, graph.direction))
        .collect::<Vec<_>>();
    match graph.direction {
        Direction::BottomUp => layout.canvas.flip_vertical(),
        Direction::RightLeft => layout.canvas.flip_horizontal(),
        Direction::TopDown | Direction::LeftRight => {}
    }
    // The conversion above already accounts for direction; retain input order.
    rects.shrink_to_fit();
    let (lines, plain_lines) = layout.canvas.to_lines(styles);
    GraphArt {
        width: layout.canvas.w,
        height: layout.canvas.h,
        lines,
        plain_lines,
        node_rects: rects,
    }
}
