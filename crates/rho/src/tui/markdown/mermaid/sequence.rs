// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use std::collections::HashMap;

use mermaid_rs_renderer::{EdgeDecoration, EdgeStyle};
use unicode_width::UnicodeWidthStr;

use crate::tui::terminal_graph::{
    draw_box, draw_seq_text, fit_label, Canvas, CellClass as Cls, GraphStyles, NodeShape, Oversize,
    Placed, D, L, MAX_CANVAS_CELLS, PAD, R, STY_SOLID, STY_THICK, U, WRAP_WIDTH,
};

use super::MermaidArt;
const SEQ_GAP: usize = 5;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum SeqHead {
    Arrow,
    Cross,
}

pub(super) enum NoteAnchor {
    Over(usize, usize),
    Left(usize),
    Right(usize),
}

pub(super) enum SeqItem {
    Message {
        from: usize,
        to: usize,
        text: Option<String>,
        dashed: bool,
        head: SeqHead,
    },
    Note {
        anchor: NoteAnchor,
        text: String,
    },
    Divider {
        text: String,
    },
}

pub(super) struct Activation {
    pub(super) participant: usize,
    pub(super) start_item: usize,
    pub(super) end_item: usize,
}

pub(super) struct Sequence {
    pub(super) labels: Vec<String>,
    pub(super) items: Vec<SeqItem>,
    pub(super) activations: Vec<Activation>,
}

pub(super) fn from_ir(ir: &mermaid_rs_renderer::Graph) -> Sequence {
    let labels = ir
        .sequence_participants
        .iter()
        .map(|id| {
            ir.nodes
                .get(id)
                .map(|node| node.label.clone())
                .unwrap_or_else(|| id.clone())
        })
        .collect::<Vec<_>>();
    let index = ir
        .sequence_participants
        .iter()
        .enumerate()
        .map(|(position, id)| (id.clone(), position))
        .collect::<HashMap<_, _>>();
    let mut items = Vec::new();
    let mut next_number = ir.sequence_autonumber;
    let mut edge_item = HashMap::new();
    for edge_index in 0..=ir.edges.len() {
        for frame in &ir.sequence_frames {
            if frame.start_idx == edge_index {
                items.push(SeqItem::Divider {
                    text: frame_divider_label(frame),
                });
            }
            for section in &frame.sections {
                if section.start_idx == edge_index && section.start_idx != frame.start_idx {
                    items.push(SeqItem::Divider {
                        text: section.label.clone().unwrap_or_else(|| "else".to_owned()),
                    });
                }
            }
        }
        for note in ir
            .sequence_notes
            .iter()
            .filter(|note| note.index == edge_index)
        {
            let participants = note
                .participants
                .iter()
                .filter_map(|id| index.get(id).copied())
                .collect::<Vec<_>>();
            let anchor = match note.position {
                mermaid_rs_renderer::ir::SequenceNotePosition::Over => {
                    let first = participants.first().copied().unwrap_or(0);
                    let last = participants.last().copied().unwrap_or(first);
                    NoteAnchor::Over(first.min(last), first.max(last))
                }
                mermaid_rs_renderer::ir::SequenceNotePosition::LeftOf => {
                    NoteAnchor::Left(participants.first().copied().unwrap_or(0))
                }
                mermaid_rs_renderer::ir::SequenceNotePosition::RightOf => {
                    NoteAnchor::Right(participants.first().copied().unwrap_or(0))
                }
            };
            items.push(SeqItem::Note {
                anchor,
                text: note.label.clone(),
            });
        }
        if let Some(edge) = ir.edges.get(edge_index) {
            if let (Some(&from), Some(&to)) = (index.get(&edge.from), index.get(&edge.to)) {
                edge_item.insert(edge_index, items.len());
                items.push(SeqItem::Message {
                    from,
                    to,
                    text: numbered_message(edge.label.clone(), &mut next_number),
                    dashed: edge.style == EdgeStyle::Dotted,
                    head: if edge.end_decoration == Some(EdgeDecoration::Cross) {
                        SeqHead::Cross
                    } else {
                        SeqHead::Arrow
                    },
                });
            }
        }
        for _frame in ir
            .sequence_frames
            .iter()
            .filter(|frame| frame.end_idx == edge_index)
        {
            items.push(SeqItem::Divider {
                text: "end".to_owned(),
            });
        }
    }
    let activations = activation_ranges(ir, &index, &edge_item, items.len());
    Sequence {
        labels,
        items,
        activations,
    }
}

fn frame_divider_label(frame: &mermaid_rs_renderer::ir::SequenceFrame) -> String {
    let kind = match frame.kind {
        mermaid_rs_renderer::ir::SequenceFrameKind::Alt => "alt",
        mermaid_rs_renderer::ir::SequenceFrameKind::Opt => "opt",
        mermaid_rs_renderer::ir::SequenceFrameKind::Loop => "loop",
        mermaid_rs_renderer::ir::SequenceFrameKind::Par => "par",
        mermaid_rs_renderer::ir::SequenceFrameKind::Rect => "rect",
        mermaid_rs_renderer::ir::SequenceFrameKind::Critical => "critical",
        mermaid_rs_renderer::ir::SequenceFrameKind::Break => "break",
    };
    match frame
        .sections
        .first()
        .and_then(|section| section.label.as_deref())
        .filter(|label| !label.is_empty())
    {
        Some(label) => format!("{kind} {label}"),
        None => kind.to_owned(),
    }
}

fn numbered_message(label: Option<String>, next_number: &mut Option<usize>) -> Option<String> {
    let Some(number) = *next_number else {
        return label;
    };
    *next_number = Some(number.saturating_add(1));
    Some(match label {
        Some(label) if !label.is_empty() => format!("{number} {label}"),
        _ => number.to_string(),
    })
}

fn activation_ranges(
    ir: &mermaid_rs_renderer::Graph,
    participants: &HashMap<String, usize>,
    edge_item: &HashMap<usize, usize>,
    item_count: usize,
) -> Vec<Activation> {
    let mut open: HashMap<usize, usize> = HashMap::new();
    let mut ranges = Vec::new();
    let mut events = ir.sequence_activations.clone();
    events.sort_by_key(|activation| activation.index);
    for activation in events {
        let Some(&participant) = participants.get(&activation.participant) else {
            continue;
        };
        let item = edge_item
            .get(&activation.index)
            .copied()
            .unwrap_or(item_count.saturating_sub(1));
        match activation.kind {
            mermaid_rs_renderer::ir::SequenceActivationKind::Activate => {
                open.entry(participant).or_insert(item);
            }
            mermaid_rs_renderer::ir::SequenceActivationKind::Deactivate => {
                if let Some(start) = open.remove(&participant) {
                    ranges.push(Activation {
                        participant,
                        start_item: start,
                        end_item: item,
                    });
                }
            }
        }
    }
    for (participant, start) in open {
        ranges.push(Activation {
            participant,
            start_item: start,
            end_item: item_count.saturating_sub(1),
        });
    }
    ranges
}

fn note_geometry(xs: &[usize], anchor: &NoteAnchor, text_w: usize) -> (usize, usize) {
    match *anchor {
        NoteAnchor::Over(l, r) => {
            let center = (xs[l] + xs[r]) / 2;
            let w = (xs[r] - xs[l] + 5).max(text_w + 2 * PAD + 2);
            (center.saturating_sub(w / 2), w)
        }
        NoteAnchor::Left(i) => {
            let w = text_w + 2 * PAD + 2;
            (xs[i].saturating_sub(2 + w - 1), w)
        }
        NoteAnchor::Right(i) => (xs[i] + 2, text_w + 2 * PAD + 2),
    }
}

pub(super) fn layout_sequence(
    seq: &Sequence,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let n = seq.labels.len();
    let labels: Vec<String> = seq
        .labels
        .iter()
        .map(|l| fit_label(l, WRAP_WIDTH))
        .collect();
    let box_w: Vec<usize> = labels
        .iter()
        .map(|l| l.width().max(1) + 2 * PAD + 2)
        .collect();
    let box_h = 3usize;

    let item_text_w = |text: &Option<String>| text.as_deref().map(|t| t.width()).unwrap_or(0);

    let mut gaps: Vec<usize> = (0..n.saturating_sub(1))
        .map(|i| SEQ_GAP.max(box_w[i].div_ceil(2) + box_w[i + 1].div_ceil(2) + 1))
        .collect();

    let mut reqs: Vec<(usize, usize, usize)> = Vec::new();
    for item in &seq.items {
        match item {
            SeqItem::Message { from, to, text, .. } => {
                let tw = item_text_w(text);
                if from != to {
                    let (l, r) = (*from.min(to), *from.max(to));
                    reqs.push((l, r, (tw + 2).max(4)));
                } else if *from + 1 < n {
                    reqs.push((*from, *from + 1, 5 + tw + 2));
                }
            }
            SeqItem::Note { anchor, text } => {
                let tw = text.width();
                match *anchor {
                    NoteAnchor::Over(l, r) if l < r => reqs.push((l, r, tw.saturating_sub(1))),
                    NoteAnchor::Over(i, _) => {
                        let half = (tw + 4).div_ceil(2) + 2;
                        if i > 0 {
                            reqs.push((i - 1, i, half));
                        }
                        if i + 1 < n {
                            reqs.push((i, i + 1, half));
                        }
                    }
                    NoteAnchor::Left(i) if i > 0 => reqs.push((i - 1, i, tw + 7)),
                    NoteAnchor::Right(i) if i + 1 < n => reqs.push((i, i + 1, tw + 7)),
                    _ => {}
                }
            }
            SeqItem::Divider { .. } => {}
        }
    }
    reqs.sort_by_key(|&(l, r, _)| r - l);
    for (l, r, need) in reqs {
        let cur: usize = gaps[l..r].iter().sum();
        if cur < need {
            gaps[r - 1] += need - cur;
        }
    }

    let mut xs = vec![0usize; n];
    xs[0] = box_w[0] / 2;
    for i in 1..n {
        xs[i] = xs[i - 1] + gaps[i - 1];
    }

    let mut canvas_w = xs[n - 1] + box_w[n - 1].div_ceil(2) + 1;
    for item in &seq.items {
        match item {
            SeqItem::Message { from, to, text, .. } if from == to => {
                canvas_w = canvas_w.max(xs[*from] + 5 + item_text_w(text) + 1);
            }
            SeqItem::Note { anchor, text } => {
                let (x, w) = note_geometry(&xs, anchor, text.width());
                canvas_w = canvas_w.max(x + w + 1);
            }
            SeqItem::Divider { text } => {
                canvas_w = canvas_w.max(text.width() + 4);
            }
            _ => {}
        }
    }

    let mut rows: Vec<usize> = Vec::with_capacity(seq.items.len());
    let mut y = box_h + 1;
    for item in &seq.items {
        rows.push(y);
        y += match item {
            SeqItem::Message { from, to, text, .. } => {
                if from == to {
                    4
                } else if text.is_some() {
                    3
                } else {
                    2
                }
            }
            SeqItem::Note { .. } => 4,
            SeqItem::Divider { .. } => 2,
        };
    }
    let bottom_top = y;
    let canvas_h = bottom_top + box_h;

    if max_width.is_some_and(|max_width| canvas_w > max_width) {
        return Err(Oversize::Width);
    }
    if canvas_w.saturating_mul(canvas_h) > MAX_CANVAS_CELLS {
        return Err(Oversize::Cells);
    }

    let mut canvas = Canvas::new(canvas_w, canvas_h);
    for i in 0..n {
        for by in [0, bottom_top] {
            let p = Placed {
                x: xs[i].saturating_sub(box_w[i] / 2),
                y: by,
                w: box_w[i],
                h: box_h,
                cx: xs[i],
                cy: by + 1,
                rank: 0,
            };
            draw_box(
                &mut canvas,
                &p,
                std::slice::from_ref(&labels[i]),
                NodeShape::Rect,
                /*node_index*/ None,
            );
        }
    }
    for (item, &r) in seq.items.iter().zip(&rows) {
        if let SeqItem::Note { anchor, text } = item {
            let (x, w) = note_geometry(&xs, anchor, text.width());
            let p = Placed {
                x,
                y: r,
                w,
                h: 3,
                cx: x + w / 2,
                cy: r + 1,
                rank: 0,
            };
            draw_box(
                &mut canvas,
                &p,
                std::slice::from_ref(text),
                NodeShape::Rect,
                /*node_index*/ None,
            );
        }
    }
    for &x in &xs {
        canvas.junction(x, box_h - 1, D);
        canvas.seg_v(x, box_h, bottom_top - 1);
        canvas.junction(x, bottom_top, U);
    }
    canvas.cur_style = STY_THICK;
    for activation in &seq.activations {
        let Some(&x) = xs.get(activation.participant) else {
            continue;
        };
        let start_y = rows.get(activation.start_item).copied().unwrap_or(box_h);
        let end_y = rows
            .get(activation.end_item)
            .copied()
            .unwrap_or(bottom_top.saturating_sub(1))
            .max(start_y);
        canvas.seg_v(x, start_y, end_y.min(bottom_top.saturating_sub(1)));
    }
    canvas.cur_style = STY_SOLID;

    for (item, &r) in seq.items.iter().zip(&rows) {
        match item {
            SeqItem::Message {
                from,
                to,
                text,
                dashed,
                head,
            } => {
                let line_ch = if *dashed { '╌' } else { '─' };
                if from == to {
                    let x = xs[*from];
                    canvas.junction(x, r, R);
                    canvas.set(x + 1, r, line_ch, Cls::Edge);
                    canvas.set(x + 2, r, line_ch, Cls::Edge);
                    canvas.set(x + 3, r, '╮', Cls::Edge);
                    canvas.set(x + 3, r + 1, '│', Cls::Edge);
                    canvas.set(
                        x + 1,
                        r + 2,
                        if *head == SeqHead::Cross { '×' } else { '◄' },
                        Cls::Edge,
                    );
                    canvas.set(x + 2, r + 2, line_ch, Cls::Edge);
                    canvas.set(x + 3, r + 2, '╯', Cls::Edge);
                    if let Some(t) = text {
                        draw_seq_text(&mut canvas, t, x + 5, r + 1, Cls::Text);
                    }
                } else {
                    let (x0, x1) = (xs[*from], xs[*to]);
                    let rightward = x1 > x0;
                    let arrow_row = if text.is_some() { r + 1 } else { r };
                    let (lo, hi) = (x0.min(x1), x0.max(x1));
                    canvas.junction(x0, arrow_row, if rightward { R } else { L });
                    for x in (lo + 1)..hi {
                        canvas.set(x, arrow_row, line_ch, Cls::Edge);
                    }
                    let head_ch = match (head, rightward) {
                        (SeqHead::Cross, _) => '×',
                        (SeqHead::Arrow, true) => '▶',
                        (SeqHead::Arrow, false) => '◄',
                    };
                    let head_x = if rightward { x1 - 1 } else { x1 + 1 };
                    canvas.set(head_x, arrow_row, head_ch, Cls::Edge);
                    if let Some(t) = text {
                        let span = hi - lo - 1;
                        let t = fit_label(t, span.max(1));
                        let tx = lo + 1 + span.saturating_sub(t.width()) / 2;
                        draw_seq_text(&mut canvas, &t, tx, r, Cls::Text);
                    }
                }
            }
            SeqItem::Note { .. } => {}
            SeqItem::Divider { text } => {
                for x in 0..canvas_w {
                    canvas.set(x, r, '─', Cls::Edge);
                }
                let t = fit_label(text, canvas_w.saturating_sub(4));
                draw_seq_text(&mut canvas, &format!(" {t} "), 2, r, Cls::EdgeLabel);
            }
        }
    }

    canvas.finalize_mask();
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
    })
}
