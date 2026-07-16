// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use unicode_width::UnicodeWidthStr;

use super::{
    canvas::{Canvas, Cls, D, L, R, U},
    flow::Placed,
    model::{Edge, Graph, Head, LineKind, Shape},
    painter::{char_width, CONT, LABEL_BREAK_CHARS, MAX_LABEL, PAD},
};
pub(super) fn wrap_label(label: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let char_w = char_width;
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
            for ch in word.chars() {
                let cw = char_w(ch);
                if chunk_w + cw > width && !chunk.is_empty() {
                    // Prefer breaking after the last identifier boundary so a long
                    // token is not sliced mid-segment; fall back to a per-char break.
                    let carry = match chunk.rfind(LABEL_BREAK_CHARS) {
                        Some(p) => chunk.split_off(p + 1),
                        None => String::new(),
                    };
                    lines.push(std::mem::take(&mut chunk));
                    chunk_w = carry.chars().map(char_w).sum();
                    chunk = carry;
                }
                chunk.push(ch);
                chunk_w += cw;
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
            for ch in last.chars() {
                let cw = char_w(ch);
                if sw + cw > target {
                    break;
                }
                s.push(ch);
                sw += cw;
            }
            s.push('…');
            *last = s;
        }
    }
    lines
}

pub(super) fn fit_label(label: &str, inner: usize) -> String {
    if label.width() <= inner {
        return label.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in label.chars() {
        let cw = char_width(c);
        if used + cw + 1 > inner {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

pub(super) fn draw_box(canvas: &mut Canvas, p: &Placed, lines: &[String], shape: Shape) {
    let (x, y, w, h) = (p.x, p.y, p.w, p.h);
    let right = x + w - 1;
    let bottom = y + h - 1;

    let (tl, tr, bl, br) = match shape {
        Shape::Round => ('╭', '╮', '╰', '╯'),
        // A diamond's corner points make decisions distinguishable from
        // rounded process nodes even in the compact rectangular cell grid.
        Shape::Diamond => ('◇', '◇', '◇', '◇'),
        Shape::Rect => ('┌', '┐', '└', '┘'),
    };
    canvas.set(x, y, tl, Cls::Border);
    canvas.set(right, y, tr, Cls::Border);
    canvas.set(x, bottom, bl, Cls::Border);
    canvas.set(right, bottom, br, Cls::Border);

    for cx in (x + 1)..right {
        canvas.add_bits(cx, y, L | R);
        canvas.add_bits(cx, bottom, L | R);
    }
    for cy in (y + 1)..bottom {
        canvas.add_bits(x, cy, U | D);
        canvas.add_bits(right, cy, U | D);
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
        for c in text.chars() {
            let cw = char_width(c);
            canvas.set(cur, row, c, Cls::Text);
            // Wide glyphs (CJK, emoji) own a second column; mark it as a
            // continuation so the line builder doesn't emit a stray space.
            for k in 1..cw {
                canvas.set(cur + k, row, CONT, Cls::Text);
            }
            cur += cw;
        }
    }
}

pub(super) fn route_forward(
    canvas: &mut Canvas,
    from: &Placed,
    to: &Placed,
    edge: &Edge,
    bus: usize,
) {
    let tx = to.cx;
    let bx = if from.cx.abs_diff(tx) <= 1 {
        tx
    } else {
        from.cx
    };
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
        place_label(canvas, label, head_row, tx + 1);
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
        _ => arrow,
    }
}

pub(super) fn route_self(canvas: &mut Canvas, p: &Placed, edge: &Edge) {
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
    canvas.set(ret_x, bottom + 1, head_glyph(edge.head_to, '▲'), Cls::Edge);
    if let Some(label) = &edge.label {
        place_label(canvas, label, bottom + 1, p.x + p.w + 1);
    }
}

pub(super) fn route_back(
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

pub(super) fn route_forward_lr(
    canvas: &mut Canvas,
    from: &Placed,
    to: &Placed,
    edge: &Edge,
    bus: usize,
) {
    let rx = from.x + from.w - 1;
    let ry = from.cy;
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

pub(super) fn route_back_lr(
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
    for c in text.chars() {
        let cw = char_width(c);
        if cw == 0 {
            canvas.set(x, row, c, Cls::EdgeLabel);
            continue;
        }
        if x + cw > canvas.w {
            break;
        }
        let blocked = (0..cw).any(|k| {
            let i = canvas.idx(x + k, row);
            canvas.ch[i] != ' ' || canvas.mask[i] != 0 || canvas.occupied[i]
        });
        if blocked {
            break;
        }
        canvas.set(x, row, c, Cls::EdgeLabel);
        for k in 1..cw {
            canvas.set(x + k, row, CONT, Cls::EdgeLabel);
        }
        x += cw;
    }
}

pub(super) fn compute_ranks(graph: &Graph) -> Vec<usize> {
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

pub(super) fn draw_seq_text(canvas: &mut Canvas, text: &str, x: usize, y: usize, cls: Cls) {
    let mut cur = x;
    for c in text.chars() {
        let cw = char_width(c);
        if cw == 0 {
            canvas.set(cur, y, c, cls);
            continue;
        }
        for k in 0..cw {
            if cur + k < canvas.w && y < canvas.h {
                let i = canvas.idx(cur + k, y);
                canvas.mask[i] = 0;
            }
            canvas.set(cur + k, y, if k == 0 { c } else { CONT }, cls);
        }
        cur += cw;
    }
}
