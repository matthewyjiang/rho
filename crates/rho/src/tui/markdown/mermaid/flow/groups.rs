use std::collections::HashMap;

use crate::tui::markdown::mermaid::{
    model::{Dir, Edge, Graph as ModelGraph, Node, Shape},
    painter::{MermaidArt, MermaidStyles, Oversize},
};
use crate::tui::terminal_graph::{self, Canvas, NodeExtra, NodeStyle};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    Node(usize),
    Group(usize),
}

pub(super) fn render_grouped(
    graph: &ModelGraph,
    styles: &MermaidStyles,
    max_width: Option<usize>,
    wrap_width: usize,
) -> Result<MermaidArt, Oversize> {
    let mut proxy: HashMap<usize, usize> = HashMap::new();
    for (group_index, group) in graph.groups.iter().enumerate() {
        if let Some(&node_index) = graph.index.get(&group.id) {
            proxy.insert(node_index, group_index);
        }
    }

    let group_chain = |group: Option<usize>| -> Vec<usize> {
        let mut chain = Vec::new();
        let mut current = group;
        while let Some(group_index) = current {
            chain.push(group_index);
            current = graph.groups[group_index].parent;
        }
        chain.reverse();
        chain
    };
    let endpoint = |node: usize| -> (Item, Vec<usize>) {
        match proxy.get(&node) {
            Some(&group) => (Item::Group(group), group_chain(graph.groups[group].parent)),
            None => (Item::Node(node), group_chain(graph.node_group[node])),
        }
    };

    let mut scope_edges: HashMap<Option<usize>, Vec<(Item, Item, usize)>> = HashMap::new();
    let mut referenced = vec![false; graph.groups.len()];
    for (edge_index, edge) in graph.edges.iter().enumerate() {
        let (from_item, from_chain) = endpoint(edge.from);
        let (to_item, to_chain) = endpoint(edge.to);
        let common = from_chain
            .iter()
            .zip(&to_chain)
            .take_while(|(from, to)| from == to)
            .count();
        let scope = (common > 0).then(|| from_chain[common - 1]);
        let from = if from_chain.len() > common {
            Item::Group(from_chain[common])
        } else {
            from_item
        };
        let to = if to_chain.len() > common {
            Item::Group(to_chain[common])
        } else {
            to_item
        };
        if let Item::Group(group) = from {
            referenced[group] = true;
        }
        if let Item::Group(group) = to {
            referenced[group] = true;
        }
        scope_edges
            .entry(scope)
            .or_default()
            .push((from, to, edge_index));
    }

    let mut direct_nodes: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (node, group) in graph.node_group.iter().enumerate() {
        if !proxy.contains_key(&node) {
            direct_nodes.entry(*group).or_default().push(node);
        }
    }
    let mut keep = vec![false; graph.groups.len()];
    for group in (0..graph.groups.len()).rev() {
        let has_nodes = direct_nodes
            .get(&Some(group))
            .is_some_and(|nodes| !nodes.is_empty());
        let has_children = (0..graph.groups.len())
            .any(|child| graph.groups[child].parent == Some(group) && keep[child]);
        keep[group] = has_nodes || has_children || referenced[group];
    }

    let mut canvas = build_scope(
        graph,
        /*scope*/ None,
        &scope_edges,
        &direct_nodes,
        &keep,
        max_width,
        wrap_width,
    )?;
    match graph.dir {
        Dir::BottomUp => canvas.flip_vertical(),
        Dir::RightLeft => canvas.flip_horizontal(),
        Dir::TopDown | Dir::LeftRight => {}
    }
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
    })
}

fn build_scope(
    graph: &ModelGraph,
    scope: Option<usize>,
    scope_edges: &HashMap<Option<usize>, Vec<(Item, Item, usize)>>,
    direct_nodes: &HashMap<Option<usize>, Vec<usize>>,
    keep: &[bool],
    max_width: Option<usize>,
    wrap_width: usize,
) -> Result<Canvas, Oversize> {
    let mut items = Vec::new();
    if let Some(nodes) = direct_nodes.get(&scope) {
        items.extend(nodes.iter().map(|&node| Item::Node(node)));
    }
    let child_groups: Vec<usize> = (0..graph.groups.len())
        .filter(|&group| graph.groups[group].parent == scope && keep[group])
        .collect();
    items.extend(child_groups.iter().map(|&group| Item::Group(group)));

    if items.is_empty() {
        return Ok(Canvas::new(1, 1));
    }

    let mut index_of = HashMap::new();
    let mut nodes = Vec::new();
    let mut extras = Vec::new();
    for item in &items {
        index_of.insert(*item, nodes.len());
        match item {
            Item::Node(node) => {
                nodes.push(Node {
                    label: graph.nodes[*node].label.clone(),
                    shape: graph.nodes[*node].shape,
                    style: graph.nodes[*node].style,
                });
                extras.push(NodeExtra::Plain);
            }
            Item::Group(group) => {
                let sub = build_scope(
                    graph,
                    Some(*group),
                    scope_edges,
                    direct_nodes,
                    keep,
                    /*max_width*/ None,
                    wrap_width,
                )?;
                nodes.push(Node {
                    label: graph.groups[*group].label.clone(),
                    shape: Shape::Rect,
                    style: NodeStyle::default(),
                });
                extras.push(NodeExtra::Frame(sub));
            }
        }
    }

    let mut edges = Vec::new();
    if let Some(list) = scope_edges.get(&scope) {
        for (from, to, edge_index) in list {
            let (Some(&from), Some(&to)) = (index_of.get(from), index_of.get(to)) else {
                continue;
            };
            let edge = &graph.edges[*edge_index];
            edges.push(Edge {
                from,
                to,
                label: edge.label.clone(),
                head_to: edge.head_to,
                head_from: edge.head_from,
                line: edge.line,
            });
        }
    }

    let direction = graph.dir;
    let synth =
        terminal_graph::Graph::from_parts(nodes, edges, direction).map_err(|_| Oversize::Cells)?;
    let layout = terminal_graph::layout_canvas(&synth, &extras, max_width, wrap_width)?;
    Ok(layout.canvas)
}
