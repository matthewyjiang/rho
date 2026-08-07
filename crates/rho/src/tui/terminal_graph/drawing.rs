// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{
    canvas::{Canvas, Cls, D, L, R, U},
    flow::Placed,
    painter::{LABEL_BREAK_CHARS, MAX_LABEL, PAD},
    Compartment, Direction, Edge, EdgeHead as Head, EdgeLine as LineKind, Graph, NodeRect,
    NodeShape as Shape, TextAlignment,
};
pub(in crate::tui) fn wrap_label(label: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in label.split_whitespace() {
        let ww = word.width();
        if ww > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            for grapheme in word.graphemes(true) {
                let grapheme_width = grapheme.width();
                if chunk_w + grapheme_width > width && !chunk.is_empty() {
                    // Prefer breaking after the last identifier boundary so a long
                    // token is not sliced mid-segment; fall back to a grapheme break.
                    let carry = match chunk.rfind(LABEL_BREAK_CHARS) {
                        Some(position) => chunk.split_off(position + 1),
                        None => String::new(),
                    };
                    lines.push(std::mem::take(&mut chunk));
                    chunk_w = carry.graphemes(true).map(UnicodeWidthStr::width).sum();
                    chunk = carry;
                }
                chunk.push_str(grapheme);
                chunk_w += grapheme_width;
            }
            cur = chunk;
            cur_w = chunk_w;
        } else if cur.is_empty() {
            cur.push_str(word);
            cur_w = ww;
        } else if cur_w + 1 + ww <= width {
            cur.push(' ');
            cur.push_str(word);
            cur_w += 1 + ww;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = ww;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            let target = width.saturating_sub(1).max(1);
            let mut s = String::new();
            let mut sw = 0usize;
            for grapheme in last.graphemes(true) {
                let grapheme_width = grapheme.width();
                if sw + grapheme_width > target {
                    break;
                }
                s.push_str(grapheme);
                sw += grapheme_width;
            }
            s.push('…');
            *last = s;
        }
    }
    lines
}

pub(in crate::tui) fn fit_label(label: &str, inner: usize) -> String {
    if label.width() <= inner {
        return label.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for grapheme in label.graphemes(true) {
        let grapheme_width = grapheme.width();
        if used + grapheme_width + 1 > inner {
            break;
        }
        out.push_str(grapheme);
        used += grapheme_width;
    }
    out.push('…');
    out
}

pub(in crate::tui) fn draw_box(
    canvas: &mut Canvas,
    p: &Placed,
    lines: &[String],
    shape: Shape,
    node_index: Option<usize>,
) {
    let (x, y, w, h) = (p.x, p.y, p.w, p.h);
    let right = x + w - 1;
    let bottom = y + h - 1;
    let border = node_index.map(Cls::NodeBorder).unwrap_or(Cls::Border);
    let text_class = node_index.map(Cls::NodeText).unwrap_or(Cls::Text);

    let (tl, tr, bl, br) = match shape {
        Shape::Round => ('╭', '╮', '╰', '╯'),
        // A diamond's corner points make decisions distinguishable from
        // rounded process nodes even in the compact rectangular cell grid.
        Shape::Diamond => ('◇', '◇', '◇', '◇'),
        Shape::Rect => ('┌', '┐', '└', '┘'),
    };
    canvas.set(x, y, tl, border);
    canvas.set(right, y, tr, border);
    canvas.set(x, bottom, bl, border);
    canvas.set(right, bottom, br, border);

    for cx in (x + 1)..right {
        canvas.add_bits_with_class(cx, y, L | R, border);
        canvas.add_bits_with_class(cx, bottom, L | R, border);
    }
    for cy in (y + 1)..bottom {
        canvas.add_bits_with_class(x, cy, U | D, border);
        canvas.add_bits_with_class(right, cy, U | D, border);
    }

    for cy in y..=bottom {
        for cx in x..=right {
            let i = canvas.idx(cx, cy);
            canvas.occupied[i] = true;
        }
    }

    let inner = w.saturating_sub(2 * PAD + 2).max(1);
    for (li, line) in lines.iter().enumerate() {
        let row = y + 1 + li;
        let text = fit_label(line, inner);
        let tw = text.width();
        let text_x = x + 1 + PAD + inner.saturating_sub(tw) / 2;
        let mut cur = text_x;
        for grapheme in text.graphemes(true) {
            cur += canvas.set_grapheme(cur, row, grapheme, text_class);
        }
    }
}

pub(in crate::tui) fn route_forward(
    canvas: &mut Canvas,
    from: &Placed,
    to: &Placed,
    edge: &Edge,
    bus: usize,
    source_anchor: usize,
) {
    let tx = to.cx;
    let bx = source_anchor;
    let by = from.y + from.h - 1;
    let head_row = to.y - 1;

    canvas.junction(bx, by, D);
    canvas.seg_v(bx, by, bus);
    if bx == tx {
        canvas.seg_v(bx, bus, head_row);
    } else {
        canvas.seg_h(bus, bx, tx);
        canvas.seg_v(tx, bus, head_row);
    }

    if edge.head_to == Head::None {
        canvas.add_bits(tx, head_row, U);
    } else {
        canvas.set(tx, head_row, head_glyph(edge.head_to, '▼'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(bx, by, head_glyph(edge.head_from, '▲'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        let label_row = head_row.saturating_sub(1);
        place_label(canvas, label, label_row, tx + 2);
    }
}

fn head_glyph(head: Head, arrow: char) -> char {
    match head {
        Head::Circle => 'o',
        Head::Cross => '×',
        Head::DiamondFill => '◆',
        Head::DiamondOpen => '◇',
        Head::Triangle => match arrow {
            '▼' => '▽',
            '▲' => '△',
            '◄' => '◁',
            '▶' => '▷',
            other => other,
        },
        Head::None | Head::Arrow => arrow,
    }
}

pub(in crate::tui) fn route_self(canvas: &mut Canvas, p: &Placed, edge: &Edge) {
    let bottom = p.y + p.h - 1;
    let exit_x = p.cx + 1;
    let ret_x = p.x + p.w - 2;
    if ret_x <= exit_x || bottom + 2 >= canvas.h {
        return;
    }
    let (v, h, bl, br) = match edge.line {
        LineKind::Dotted => ('╎', '╌', '╰', '╯'),
        LineKind::Thick => ('┃', '━', '┗', '┛'),
        LineKind::Solid => ('│', '─', '╰', '╯'),
    };
    canvas.junction(exit_x, bottom, D);
    canvas.set(exit_x, bottom + 1, v, Cls::Edge);
    canvas.set(exit_x, bottom + 2, bl, Cls::Edge);
    for x in (exit_x + 1)..ret_x {
        canvas.set(x, bottom + 2, h, Cls::Edge);
    }
    canvas.set(ret_x, bottom + 2, br, Cls::Edge);
    if edge.head_to == Head::None {
        canvas.set(ret_x, bottom + 1, v, Cls::Edge);
    } else {
        canvas.set(ret_x, bottom + 1, head_glyph(edge.head_to, '▲'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(exit_x, bottom, head_glyph(edge.head_from, '▲'), Cls::Edge);
    }
    if let Some(label) = &edge.label {
        place_label(canvas, label, bottom + 1, p.x + p.w + 1);
    }
}

pub(in crate::tui) fn route_back(
    canvas: &mut Canvas,
    from: &Placed,
    to: &Placed,
    edge: &Edge,
    lane_x: usize,
) {
    let sx = from.x + from.w - 1;
    let sy = from.cy;
    let tx = to.x + to.w - 1;
    let tyc = to.cy;

    canvas.junction(sx, sy, R);
    canvas.seg_h(sy, sx, lane_x);
    canvas.seg_v(lane_x, sy, tyc);
    canvas.seg_h(tyc, tx + 1, lane_x);

    if edge.head_to == Head::None {
        canvas.add_bits(tx + 1, tyc, R);
    } else {
        canvas.set(tx + 1, tyc, head_glyph(edge.head_to, '◄'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(sx, sy, head_glyph(edge.head_from, '◄'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(
            canvas,
            label,
            tyc.saturating_sub(1),
            lane_x.saturating_sub(label.width() + 1),
        );
    }
}

pub(in crate::tui) fn route_forward_lr(
    canvas: &mut Canvas,
    from: &Placed,
    to: &Placed,
    edge: &Edge,
    bus: usize,
    source_anchor: usize,
) {
    let rx = from.x + from.w - 1;
    let ry = source_anchor;
    let ly = to.cy;
    let head_col = to.x - 1;

    canvas.junction(rx, ry, R);
    canvas.seg_h(ry, rx, bus);
    if ry == ly {
        canvas.seg_h(ry, bus, head_col);
    } else {
        canvas.seg_v(bus, ry, ly);
        canvas.seg_h(ly, bus, head_col);
    }

    if edge.head_to == Head::None {
        canvas.add_bits(head_col, ly, R);
    } else {
        canvas.set(head_col, ly, head_glyph(edge.head_to, '▶'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(rx, ry, head_glyph(edge.head_from, '◄'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(canvas, label, ly.saturating_sub(1), bus + 1);
    }
}

pub(in crate::tui) fn route_back_lr(
    canvas: &mut Canvas,
    from: &Placed,
    to: &Placed,
    edge: &Edge,
    lane_y: usize,
) {
    let sx = from.cx;
    let sy = from.y + from.h - 1;
    let tx = to.cx;
    let ty = to.y + to.h - 1;

    canvas.junction(sx, sy, D);
    canvas.seg_v(sx, sy, lane_y);
    canvas.seg_h(lane_y, sx, tx);
    canvas.seg_v(tx, lane_y, ty + 1);

    if edge.head_to == Head::None {
        canvas.add_bits(tx, ty + 1, D);
    } else {
        canvas.set(tx, ty + 1, head_glyph(edge.head_to, '▲'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(sx, sy, head_glyph(edge.head_from, '▲'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(canvas, label, lane_y.saturating_sub(1), (sx + tx) / 2);
    }
}

fn place_label(canvas: &mut Canvas, label: &str, row: usize, start_x: usize) {
    if row >= canvas.h {
        return;
    }
    let text = fit_label(label, MAX_LABEL);
    let mut x = start_x;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if grapheme_width == 0 {
            canvas.set_grapheme(x, row, grapheme, Cls::EdgeLabel);
            continue;
        }
        if x + grapheme_width > canvas.w {
            break;
        }
        let blocked = (0..grapheme_width).any(|offset| {
            let index = canvas.idx(x + offset, row);
            canvas.ch[index] != ' ' || canvas.mask[index] != 0 || canvas.occupied[index]
        });
        if blocked {
            break;
        }
        canvas.set_grapheme(x, row, grapheme, Cls::EdgeLabel);
        x += grapheme_width;
    }
}

pub(in crate::tui) fn compute_ranks(graph: &Graph) -> Vec<usize> {
    let n = graph.nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for e in &graph.edges {
        if e.from != e.to {
            children[e.from].push(e.to);
            indeg[e.to] += 1;
        }
    }

    let mut color = vec![0u8; n];
    let mut dag: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut order: Vec<usize> = Vec::with_capacity(n);

    let roots: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    for start in roots.iter().copied().chain(0..n) {
        if color[start] == 0 {
            dfs_dag(start, &children, &mut color, &mut dag, &mut order);
        }
    }

    let mut rank = vec![0usize; n];
    for &u in order.iter().rev() {
        for &v in &dag[u] {
            rank[v] = rank[v].max(rank[u] + 1);
        }
    }
    rank
}

fn dfs_dag(
    start: usize,
    children: &[Vec<usize>],
    color: &mut [u8],
    dag: &mut [Vec<usize>],
    order: &mut Vec<usize>,
) {
    let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
    color[start] = 1;
    while let Some(frame) = stack.last_mut() {
        let u = frame.0;
        if frame.1 < children[u].len() {
            let v = children[u][frame.1];
            frame.1 += 1;
            if color[v] == 1 {
                continue;
            }
            dag[u].push(v);
            if color[v] == 0 {
                color[v] = 1;
                stack.push((v, 0));
            }
        } else {
            color[u] = 2;
            order.push(u);
            stack.pop();
        }
    }
}

pub(in crate::tui) fn draw_seq_text(canvas: &mut Canvas, text: &str, x: usize, y: usize, cls: Cls) {
    let mut current = x;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        for offset in 0..grapheme_width {
            if current + offset < canvas.w && y < canvas.h {
                let index = canvas.idx(current + offset, y);
                canvas.mask[index] = 0;
            }
        }
        current += canvas.set_grapheme(current, y, grapheme, cls);
    }
}
pub(in crate::tui) fn draw_compartment_box(
    canvas: &mut Canvas,
    placed: &Placed,
    compartments: &[Compartment],
    node_index: Option<usize>,
) {
    draw_box(canvas, placed, &[], Shape::Rect, node_index);
    let inner = placed.w.saturating_sub(2 * PAD + 2).max(1);
    let border = node_index.map(Cls::NodeBorder).unwrap_or(Cls::Border);
    let text_class = node_index.map(Cls::NodeText).unwrap_or(Cls::Text);
    let mut row = placed.y + 1;
    let mut first = true;
    for compartment in compartments {
        if compartment.lines.is_empty() {
            continue;
        }
        if !first {
            canvas.set(placed.x, row, '├', border);
            for x in (placed.x + 1)..(placed.x + placed.w - 1) {
                canvas.set(x, row, '─', border);
            }
            canvas.set(placed.x + placed.w - 1, row, '┤', border);
            row += 1;
        }
        first = false;
        for line in &compartment.lines {
            let text = fit_label(line, inner);
            let x = match compartment.alignment {
                TextAlignment::Left => placed.x + 1 + PAD,
                TextAlignment::Center => {
                    placed.x + 1 + PAD + inner.saturating_sub(text.width()) / 2
                }
            };
            draw_seq_text(canvas, &text, x, row, text_class);
            row += 1;
        }
    }
}

pub(in crate::tui) fn draw_frame(
    canvas: &mut Canvas,
    placed: &Placed,
    title: &str,
    sub: &Canvas,
    node_index: Option<usize>,
) {
    draw_box(canvas, placed, &[], Shape::Rect, node_index);
    let text_class = node_index.map(Cls::NodeText).unwrap_or(Cls::Text);
    let title = fit_label(title, placed.w.saturating_sub(4));
    draw_seq_text(
        canvas,
        &format!(" {title} "),
        placed.x + 1,
        placed.y,
        text_class,
    );
    let ox = placed.x + 1 + (placed.w - 2 - sub.w) / 2;
    let oy = placed.y + 1 + (placed.h - 2 - sub.h) / 2;
    canvas.blit(sub, ox, oy);
}

pub(in crate::tui) fn art_node_rect(
    placed: Placed,
    canvas_w: usize,
    canvas_h: usize,
    direction: Direction,
) -> NodeRect {
    let (x, y) = match direction {
        Direction::BottomUp => (placed.x, canvas_h.saturating_sub(placed.y + placed.h)),
        Direction::RightLeft => (canvas_w.saturating_sub(placed.x + placed.w), placed.y),
        Direction::TopDown | Direction::LeftRight => (placed.x, placed.y),
    };
    NodeRect {
        x,
        y,
        width: placed.w,
        height: placed.h,
    }
}
