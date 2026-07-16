use std::collections::HashMap;

use mermaid_rs_renderer::{
    DiagramKind, Direction, EdgeArrowhead, EdgeDecoration, EdgeStyle, NodeShape,
};
use unicode_width::UnicodeWidthStr;

use super::{
    drawing::wrap_label,
    painter::{MAX_LABEL, MAX_LINES, WRAP_WIDTH},
    policy::{diagram_policy, DiagramPolicy},
    sequence::{NoteAnchor, SeqHead, SeqItem, Sequence},
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Shape {
    Rect,
    Round,
    Diamond,
}

pub(super) struct Node {
    pub(super) label: String,
    pub(super) shape: Shape,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Head {
    None,
    Arrow,
    Circle,
    Cross,
    Triangle,
    DiamondFill,
    DiamondOpen,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum LineKind {
    Solid,
    Dotted,
    Thick,
}

pub(super) struct Edge {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) label: Option<String>,
    pub(super) head_to: Head,
    pub(super) head_from: Head,
    pub(super) line: LineKind,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Dir {
    Down,
    Up,
    Right,
    Left,
}

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
    pub(super) dir: Dir,
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
    let edges = ir
        .edges
        .iter()
        .filter_map(|edge| {
            Some(Edge {
                from: *index.get(&edge.from)?,
                to: *index.get(&edge.to)?,
                label: if ir.kind == DiagramKind::Er {
                    er_label(edge)
                } else {
                    edge.label.clone()
                },
                head_to: edge_head(edge.arrow_end, edge.arrow_end_kind, edge.end_decoration),
                head_from: edge_head(
                    edge.arrow_start,
                    edge.arrow_start_kind,
                    edge.start_decoration,
                ),
                line: match edge.style {
                    EdgeStyle::Solid => LineKind::Solid,
                    EdgeStyle::Dotted => LineKind::Dotted,
                    EdgeStyle::Thick => LineKind::Thick,
                },
            })
        })
        .collect();
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

fn shape(shape: NodeShape) -> Shape {
    match shape {
        NodeShape::Diamond | NodeShape::Hexagon => Shape::Diamond,
        NodeShape::RoundRect
        | NodeShape::Stadium
        | NodeShape::Circle
        | NodeShape::DoubleCircle
        | NodeShape::ActorBox => Shape::Round,
        NodeShape::Rectangle
        | NodeShape::ForkJoin
        | NodeShape::Subroutine
        | NodeShape::Cylinder
        | NodeShape::Parallelogram
        | NodeShape::ParallelogramAlt
        | NodeShape::Trapezoid
        | NodeShape::TrapezoidAlt
        | NodeShape::Asymmetric
        | NodeShape::MindmapDefault
        | NodeShape::Text => Shape::Rect,
    }
}

fn direction(direction: Direction) -> Dir {
    match direction {
        Direction::TopDown => Dir::Down,
        Direction::BottomTop => Dir::Up,
        Direction::LeftRight => Dir::Right,
        Direction::RightLeft => Dir::Left,
    }
}

fn edge_head(
    arrow: bool,
    arrowhead: Option<EdgeArrowhead>,
    decoration: Option<EdgeDecoration>,
) -> Head {
    match decoration {
        Some(EdgeDecoration::Circle) => Head::Circle,
        Some(EdgeDecoration::Cross) => Head::Cross,
        Some(EdgeDecoration::Diamond) => Head::DiamondOpen,
        Some(EdgeDecoration::DiamondFilled) => Head::DiamondFill,
        // Grok's compact painter uses textual cardinality labels for these.
        // The public IR keeps the relationship semantics, and the closest
        // unambiguous terminal endpoint is an open circle or plain line.
        Some(EdgeDecoration::CrowsFootZeroOne | EdgeDecoration::CrowsFootZeroMany) => Head::Circle,
        Some(EdgeDecoration::CrowsFootOne | EdgeDecoration::CrowsFootMany) => Head::None,
        None if matches!(arrowhead, Some(EdgeArrowhead::OpenTriangle)) => Head::Triangle,
        None if arrow || matches!(arrowhead, Some(EdgeArrowhead::ClassDependency)) => Head::Arrow,
        None => Head::None,
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
    for edge_index in 0..=ir.edges.len() {
        for frame in &ir.sequence_frames {
            if frame.start_idx == edge_index {
                items.push(SeqItem::Divider {
                    text: format!("{:?}", frame.kind).to_ascii_lowercase(),
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
                items.push(SeqItem::Message {
                    from,
                    to,
                    text: edge.label.clone(),
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
    Sequence { labels, items }
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

/// Return true only when every semantic field consumed from the public IR has
/// an unambiguous, lossless representation in the terminal painter.
pub(super) fn can_paint_losslessly(ir: &mermaid_rs_renderer::Graph) -> bool {
    if !ir.class_defs.is_empty()
        || !ir.node_classes.is_empty()
        || !ir.node_styles.is_empty()
        || !ir.subgraph_styles.is_empty()
        || !ir.subgraph_classes.is_empty()
        || !ir.edge_styles.is_empty()
        || ir.edge_style_default.is_some()
    {
        return false;
    }

    let mut directed_pairs = std::collections::HashSet::new();
    for edge in &ir.edges {
        if !ir.nodes.contains_key(&edge.from)
            || !ir.nodes.contains_key(&edge.to)
            || edge.start_label.is_some()
            || edge.end_label.is_some()
        {
            return false;
        }
        if !directed_pairs.insert((&edge.from, &edge.to)) {
            // The compact router shares tracks for parallel relations, which
            // can merge both routes and their labels.
            return false;
        }
        if edge.from == edge.to
            && (edge.arrow_start
                || edge.start_decoration.is_some()
                || edge.end_decoration.is_some())
        {
            // The self-loop painter has only one endpoint attachment slot.
            return false;
        }
        if edge
            .label
            .as_deref()
            .is_some_and(|label| label.width() > MAX_LABEL)
        {
            return false;
        }
    }

    match diagram_policy(ir.kind) {
        DiagramPolicy::PaintFlow => {
            ir.nodes.values().all(|node| {
                matches!(
                    node.shape,
                    NodeShape::Rectangle | NodeShape::RoundRect | NodeShape::Diamond
                ) && plain_label_fits(&node.label)
            }) && ir
                .subgraphs
                .iter()
                .all(|group| group.label.width() <= WRAP_WIDTH)
        }
        DiagramPolicy::PaintState => {
            ir.state_notes.is_empty()
                && ir.nodes.values().all(|node| {
                    matches!(node.shape, NodeShape::Rectangle | NodeShape::RoundRect)
                        && !node.label.contains("\n---")
                        && plain_label_fits(&node.label)
                })
        }
        DiagramPolicy::PaintClass => {
            ir.nodes
                .values()
                .all(|node| node.shape == NodeShape::Rectangle)
                && ir
                    .edges
                    .iter()
                    .all(|edge| edge.start_label.is_none() && edge.end_label.is_none())
        }
        DiagramPolicy::PaintEr => ir
            .nodes
            .values()
            .all(|node| matches!(node.shape, NodeShape::Rectangle | NodeShape::RoundRect)),
        DiagramPolicy::PaintSequence => {
            !ir.sequence_participants.is_empty()
                && ir.sequence_activations.is_empty()
                && ir.sequence_autonumber.is_none()
                && ir.sequence_boxes.is_empty()
                && ir.sequence_frames.is_empty()
                && ir.sequence_participants.iter().all(|id| {
                    ir.nodes
                        .get(id)
                        .map(|node| {
                            node.shape == NodeShape::ActorBox && node.label.width() <= WRAP_WIDTH
                        })
                        .unwrap_or(false)
                })
                && ir.sequence_notes.iter().all(|note| {
                    !note.participants.is_empty()
                        && note
                            .participants
                            .iter()
                            .all(|id| ir.sequence_participants.contains(id))
                        && note.label.width() <= MAX_LABEL
                })
        }
        DiagramPolicy::RawFallback => false,
    }
}

fn plain_label_fits(label: &str) -> bool {
    wrap_label(label, WRAP_WIDTH, usize::MAX).len() <= MAX_LINES
}
