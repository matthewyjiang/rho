use std::collections::HashMap;

use mermaid_rs_renderer::{
    DiagramKind, Direction as MermaidDirection, EdgeArrowhead, EdgeDecoration, EdgeStyle,
    NodeShape as MermaidNodeShape,
};

use crate::tui::terminal_graph::{
    wrap_label, Direction, Edge, EdgeHead, EdgeLine, Node, NodeShape, NodeStyle, RankOrdering,
    MAX_LINES, WRAP_WIDTH,
};

use super::{
    policy::{diagram_policy, DiagramPolicy},
    sequence::{NoteAnchor, SeqHead, SeqItem, Sequence},
};

pub(super) struct Group {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) parent: Option<usize>,
}

pub(super) struct Graph {
    pub(super) nodes: Vec<Node>,
    pub(super) edges: Vec<Edge>,
    pub(super) index: HashMap<String, usize>,
    pub(super) groups: Vec<Group>,
    pub(super) node_group: Vec<Option<usize>>,
    pub(super) dir: Direction,
}

impl Graph {
    pub(super) fn layout_graph(&self) -> crate::tui::terminal_graph::Graph {
        crate::tui::terminal_graph::Graph::from_parts(
            self.nodes.clone(),
            self.edges.clone(),
            self.dir,
            RankOrdering::MinimizeCrossings,
        )
        .expect("Mermaid model validates edge endpoints before layout")
    }
}

pub(super) struct ClassInfo {
    pub(super) annotations: Vec<String>,
    pub(super) attrs: Vec<String>,
    pub(super) methods: Vec<String>,
}

pub(super) struct TerminalModel {
    pub(super) graph: Graph,
    pub(super) class_info: Option<Vec<ClassInfo>>,
    pub(super) sequence: Option<Sequence>,
}

pub(super) fn from_ir(ir: &mermaid_rs_renderer::Graph) -> Option<TerminalModel> {
    if diagram_policy(ir.kind) == DiagramPolicy::RawFallback {
        return None;
    }

    let mut ids = ir.nodes.keys().collect::<Vec<_>>();
    ids.sort_by_key(|id| ir.node_order.get(*id).copied().unwrap_or(usize::MAX));
    let index = ids
        .iter()
        .enumerate()
        .map(|(position, id)| ((*id).clone(), position))
        .collect::<HashMap<_, _>>();
    let groups = ir
        .subgraphs
        .iter()
        .enumerate()
        .map(|(position, group)| Group {
            id: group
                .id
                .clone()
                .unwrap_or_else(|| format!("group-{position}")),
            label: group.label.clone(),
            // mermaid-rs-renderer exposes resolved node membership rather than
            // a second subgraph tree. Flat ownership preserves semantic groups
            // without attempting to reparse declarations.
            parent: None,
        })
        .collect::<Vec<_>>();
    let nodes = ids
        .iter()
        .map(|id| {
            let node = &ir.nodes[*id];
            let mut label = match ir.kind {
                DiagramKind::Class | DiagramKind::Er => node
                    .label
                    .lines()
                    .find(|line| !line.starts_with("<<") && *line != "---")
                    .unwrap_or(&node.label)
                    .to_owned(),
                _ => node.label.clone(),
            };
            if ir.kind == DiagramKind::State {
                for note in ir.state_notes.iter().filter(|note| note.target == **id) {
                    label.push_str("\n(note: ");
                    label.push_str(&note.label);
                    label.push(')');
                }
            }
            Node {
                label,
                shape: shape(node.shape),
                style: NodeStyle::default(),
            }
        })
        .collect::<Vec<_>>();
    let node_group = ids
        .iter()
        .map(|id| {
            ir.subgraphs
                .iter()
                .position(|group| group.nodes.contains(id))
        })
        .collect();
    let edges = merge_parallel_edges(
        ir.edges
            .iter()
            .filter_map(|edge| {
                Some(Edge {
                    from: *index.get(&edge.from)?,
                    to: *index.get(&edge.to)?,
                    label: composed_edge_label(ir.kind, edge),
                    head_to: edge_head(edge.arrow_end, edge.arrow_end_kind, edge.end_decoration),
                    head_from: edge_head(
                        edge.arrow_start,
                        edge.arrow_start_kind,
                        edge.start_decoration,
                    ),
                    line: match edge.style {
                        EdgeStyle::Solid => EdgeLine::Solid,
                        EdgeStyle::Dotted => EdgeLine::Dotted,
                        EdgeStyle::Thick => EdgeLine::Thick,
                    },
                })
            })
            .collect(),
    );
    let graph = Graph {
        nodes,
        edges,
        index,
        groups,
        node_group,
        dir: direction(ir.direction),
    };

    let class_info =
        matches!(ir.kind, DiagramKind::Class | DiagramKind::Er).then(|| class_info(ir, &ids));
    let sequence = (ir.kind == DiagramKind::Sequence).then(|| sequence(ir));
    Some(TerminalModel {
        graph,
        class_info,
        sequence,
    })
}

fn shape(shape: MermaidNodeShape) -> NodeShape {
    match shape {
        MermaidNodeShape::Diamond | MermaidNodeShape::Hexagon => NodeShape::Diamond,
        MermaidNodeShape::RoundRect
        | MermaidNodeShape::Stadium
        | MermaidNodeShape::Circle
        | MermaidNodeShape::DoubleCircle
        | MermaidNodeShape::ActorBox => NodeShape::Round,
        MermaidNodeShape::Rectangle
        | MermaidNodeShape::ForkJoin
        | MermaidNodeShape::Subroutine
        | MermaidNodeShape::Cylinder
        | MermaidNodeShape::Parallelogram
        | MermaidNodeShape::ParallelogramAlt
        | MermaidNodeShape::Trapezoid
        | MermaidNodeShape::TrapezoidAlt
        | MermaidNodeShape::Asymmetric
        | MermaidNodeShape::MindmapDefault
        | MermaidNodeShape::Text => NodeShape::Rect,
    }
}

fn direction(direction: MermaidDirection) -> Direction {
    match direction {
        MermaidDirection::TopDown => Direction::TopDown,
        MermaidDirection::BottomTop => Direction::BottomUp,
        MermaidDirection::LeftRight => Direction::LeftRight,
        MermaidDirection::RightLeft => Direction::RightLeft,
    }
}

fn edge_head(
    arrow: bool,
    arrowhead: Option<EdgeArrowhead>,
    decoration: Option<EdgeDecoration>,
) -> EdgeHead {
    match decoration {
        Some(EdgeDecoration::Circle) => EdgeHead::Circle,
        Some(EdgeDecoration::Cross) => EdgeHead::Cross,
        Some(EdgeDecoration::Diamond) => EdgeHead::DiamondOpen,
        Some(EdgeDecoration::DiamondFilled) => EdgeHead::DiamondFill,
        // Grok's compact painter uses textual cardinality labels for these.
        // The public IR keeps the relationship semantics, and the closest
        // unambiguous terminal endpoint is an open circle or plain line.
        Some(EdgeDecoration::CrowsFootZeroOne | EdgeDecoration::CrowsFootZeroMany) => {
            EdgeHead::Circle
        }
        Some(EdgeDecoration::CrowsFootOne | EdgeDecoration::CrowsFootMany) => EdgeHead::None,
        None if matches!(arrowhead, Some(EdgeArrowhead::OpenTriangle)) => EdgeHead::Triangle,
        None if arrow || matches!(arrowhead, Some(EdgeArrowhead::ClassDependency)) => {
            EdgeHead::Arrow
        }
        None => EdgeHead::None,
    }
}

fn er_label(edge: &mermaid_rs_renderer::Edge) -> Option<String> {
    let start = cardinality(edge.start_decoration);
    let end = cardinality(edge.end_decoration);
    let relationship = edge.label.as_deref().unwrap_or_default();
    let label = [start, relationship, end]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!label.is_empty()).then_some(label)
}

fn cardinality(decoration: Option<EdgeDecoration>) -> &'static str {
    match decoration {
        Some(EdgeDecoration::CrowsFootOne) => "1",
        Some(EdgeDecoration::CrowsFootZeroOne) => "0..1",
        Some(EdgeDecoration::CrowsFootMany) => "1..*",
        Some(EdgeDecoration::CrowsFootZeroMany) => "0..*",
        _ => "",
    }
}

fn class_info(ir: &mermaid_rs_renderer::Graph, ids: &[&String]) -> Vec<ClassInfo> {
    ids.iter()
        .map(|id| {
            let mut annotations = Vec::new();
            let mut title_seen = false;
            let mut compartment = 0usize;
            let mut attrs = Vec::new();
            let mut method_lines = Vec::new();
            for line in ir.nodes[*id].label.lines() {
                if !title_seen && line.starts_with("<<") && line.ends_with(">>") {
                    annotations.push(line.trim_matches(&['<', '>'][..]).to_owned());
                } else if !title_seen {
                    title_seen = true;
                } else if line == "---" {
                    compartment += 1;
                } else if compartment >= 2
                    || (compartment == 1 && line.contains('(') && line.contains(')'))
                {
                    method_lines.push(line.to_owned());
                } else {
                    attrs.push(line.to_owned());
                }
            }
            ClassInfo {
                annotations,
                attrs,
                methods: method_lines,
            }
        })
        .collect()
}

fn sequence(ir: &mermaid_rs_renderer::Graph) -> Sequence {
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
        for frame in ir
            .sequence_frames
            .iter()
            .filter(|frame| frame.end_idx == edge_index)
        {
            let _ = frame;
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
) -> Vec<super::sequence::Activation> {
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
                    ranges.push(super::sequence::Activation {
                        participant,
                        start_item: start,
                        end_item: item,
                    });
                }
            }
        }
    }
    for (participant, start) in open {
        ranges.push(super::sequence::Activation {
            participant,
            start_item: start,
            end_item: item_count.saturating_sub(1),
        });
    }
    ranges
}

pub(super) fn complexity(ir: &mermaid_rs_renderer::Graph) -> (usize, usize, usize, usize) {
    let details = match diagram_policy(ir.kind) {
        DiagramPolicy::PaintSequence => {
            ir.sequence_notes.len() + ir.sequence_frames.len() + ir.sequence_activations.len()
        }
        DiagramPolicy::PaintClass | DiagramPolicy::PaintEr => ir
            .nodes
            .values()
            .map(|node| node.label.lines().count().saturating_sub(1))
            .sum(),
        DiagramPolicy::PaintFlow | DiagramPolicy::PaintState | DiagramPolicy::RawFallback => 0,
    };
    (ir.nodes.len(), ir.edges.len(), ir.subgraphs.len(), details)
}

/// Return true when the terminal painter can draw this IR without dropping
/// structure. Cosmetic styles are ignored. Parallel edges, long labels, and
/// common shapes are approximated instead of refused.
pub(super) fn can_paint(ir: &mermaid_rs_renderer::Graph) -> bool {
    for edge in &ir.edges {
        if !ir.nodes.contains_key(&edge.from) || !ir.nodes.contains_key(&edge.to) {
            return false;
        }
        if !plain_label_fits(&composed_edge_label(ir.kind, edge).unwrap_or_default()) {
            return false;
        }
    }

    match diagram_policy(ir.kind) {
        DiagramPolicy::PaintFlow => {
            ir.nodes.values().all(|node| plain_label_fits(&node.label))
                && ir
                    .subgraphs
                    .iter()
                    .all(|group| plain_label_fits(&group.label))
        }
        DiagramPolicy::PaintState => ir
            .nodes
            .values()
            .all(|node| !node.label.contains("\n---") && plain_label_fits(&node.label)),
        DiagramPolicy::PaintClass | DiagramPolicy::PaintEr => {
            ir.nodes.values().all(|node| plain_label_fits(&node.label))
        }
        DiagramPolicy::PaintSequence => {
            !ir.sequence_participants.is_empty()
                && ir.sequence_participants.iter().all(|id| {
                    ir.nodes.get(id).is_some_and(|node| {
                        node.shape == MermaidNodeShape::ActorBox && plain_label_fits(&node.label)
                    })
                })
                && ir.sequence_notes.iter().all(|note| {
                    !note.participants.is_empty()
                        && note
                            .participants
                            .iter()
                            .all(|id| ir.sequence_participants.contains(id))
                        && plain_label_fits(&note.label)
                })
        }
        DiagramPolicy::RawFallback => false,
    }
}

fn composed_edge_label(kind: DiagramKind, edge: &mermaid_rs_renderer::Edge) -> Option<String> {
    if kind == DiagramKind::Er {
        return er_label(edge);
    }
    let label = [
        edge.start_label.as_deref().unwrap_or(""),
        edge.label.as_deref().unwrap_or(""),
        edge.end_label.as_deref().unwrap_or(""),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    (!label.is_empty()).then_some(label)
}

fn merge_parallel_edges(edges: Vec<Edge>) -> Vec<Edge> {
    let mut merged: Vec<Edge> = Vec::new();
    let mut index: HashMap<(usize, usize), usize> = HashMap::new();
    for edge in edges {
        if let Some(&existing) = index.get(&(edge.from, edge.to)) {
            let current = merged[existing].label.take();
            merged[existing].label = merge_labels(current, edge.label);
            continue;
        }
        index.insert((edge.from, edge.to), merged.len());
        merged.push(edge);
    }
    merged
}

fn merge_labels(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (None, label) | (label, None) => label,
        (Some(left), Some(right)) if left == right => Some(left),
        (Some(left), Some(right)) => Some(format!("{left} / {right}")),
    }
}

fn plain_label_fits(label: &str) -> bool {
    wrap_label(label, WRAP_WIDTH, usize::MAX).len() <= MAX_LINES
}
