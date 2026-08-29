use std::collections::HashMap;

use mermaid_rs_renderer::{
    DiagramKind, Direction as MermaidDirection, EdgeArrowhead, EdgeDecoration, EdgeStyle,
    NodeShape as MermaidNodeShape,
};

use crate::tui::terminal_graph::{
    Direction, Edge, EdgeHead, EdgeLine, Node, NodeShape, NodeStyle, RankOrdering,
};

use super::{
    gantt, gitgraph, mindmap,
    policy::{diagram_policy, DiagramPolicy},
    sequence::Sequence,
};

#[derive(Clone)]
pub(super) struct Group {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) parent: Option<usize>,
}

#[derive(Clone)]
pub(super) struct Graph {
    pub(super) nodes: Vec<Node>,
    pub(super) edges: Vec<Edge>,
    pub(super) index: HashMap<String, usize>,
    pub(super) groups: Vec<Group>,
    pub(super) node_group: Vec<Option<usize>>,
    pub(super) dir: Direction,
}

impl Graph {
    pub(super) fn with_dir(&self, dir: Direction) -> Self {
        Self {
            dir,
            ..self.clone()
        }
    }

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

pub(super) enum TerminalModel {
    Flow(Graph),
    Class { graph: Graph, info: Vec<ClassInfo> },
    Sequence(Sequence),
    GitGraph(gitgraph::GitGraphModel),
    Gantt(gantt::GanttModel),
    Mindmap(mindmap::MindmapModel),
}

pub(super) fn from_ir(ir: &mermaid_rs_renderer::Graph) -> Option<TerminalModel> {
    match diagram_policy(ir.kind) {
        DiagramPolicy::RawFallback => None,
        DiagramPolicy::PaintGitGraph => gitgraph::from_ir(ir).map(TerminalModel::GitGraph),
        DiagramPolicy::PaintGantt => gantt::from_ir(ir).map(TerminalModel::Gantt),
        DiagramPolicy::PaintMindmap => mindmap::from_ir(ir).map(TerminalModel::Mindmap),
        DiagramPolicy::PaintSequence => {
            let sequence = super::sequence::from_ir(ir);
            if sequence.labels.is_empty() {
                None
            } else {
                Some(TerminalModel::Sequence(sequence))
            }
        }
        DiagramPolicy::PaintFlow | DiagramPolicy::PaintState => {
            Some(TerminalModel::Flow(flow_graph(ir)))
        }
        DiagramPolicy::PaintClass | DiagramPolicy::PaintEr => {
            let (graph, ids) = flow_graph_with_ids(ir);
            Some(TerminalModel::Class {
                info: class_info(ir, &ids),
                graph,
            })
        }
    }
}

fn flow_graph(ir: &mermaid_rs_renderer::Graph) -> Graph {
    flow_graph_with_ids(ir).0
}

fn flow_graph_with_ids(ir: &mermaid_rs_renderer::Graph) -> (Graph, Vec<&String>) {
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
    (
        Graph {
            nodes,
            edges,
            index,
            groups,
            node_group,
            dir: direction(ir.direction),
        },
        ids,
    )
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

pub(super) fn complexity(ir: &mermaid_rs_renderer::Graph) -> (usize, usize, usize, usize) {
    match diagram_policy(ir.kind) {
        DiagramPolicy::PaintSequence => (
            ir.nodes.len(),
            ir.edges.len(),
            ir.subgraphs.len(),
            ir.sequence_notes.len() + ir.sequence_frames.len() + ir.sequence_activations.len(),
        ),
        DiagramPolicy::PaintClass | DiagramPolicy::PaintEr => (
            ir.nodes.len(),
            ir.edges.len(),
            ir.subgraphs.len(),
            ir.nodes
                .values()
                .map(|node| node.label.lines().count().saturating_sub(1))
                .sum(),
        ),
        DiagramPolicy::PaintGitGraph => (
            ir.gitgraph.commits.len(),
            ir.gitgraph
                .commits
                .iter()
                .map(|commit| commit.parents.len())
                .sum(),
            ir.gitgraph.branches.len(),
            ir.gitgraph
                .commits
                .iter()
                .map(|commit| commit.tags.len())
                .sum(),
        ),
        DiagramPolicy::PaintGantt => (
            ir.gantt_tasks.len(),
            ir.edges.len(),
            ir.gantt_sections.len(),
            0,
        ),
        DiagramPolicy::PaintMindmap => (ir.mindmap.nodes.len(), ir.edges.len(), 0, 0),
        DiagramPolicy::PaintFlow | DiagramPolicy::PaintState | DiagramPolicy::RawFallback => {
            (ir.nodes.len(), ir.edges.len(), ir.subgraphs.len(), 0)
        }
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

/// The compact router shares one track per directed pair. Join labels onto the
/// first edge and drop later geometry rather than refusing the diagram.
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
